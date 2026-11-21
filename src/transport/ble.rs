//! Bluetooth LE transport for ZMK Studio.
//!
//! ZMK exposes the RPC protocol over a custom GATT service. Its advertising
//! data only contains the standard HID and battery service UUIDs, so an
//! already-connected keyboard (the normal case on a desktop) is found by
//! asking the OS for connected peripherals that expose the Studio service.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bluest::{Adapter, Characteristic, Device, DeviceId, Uuid};
use futures::StreamExt;

use super::{Connection, ConnectionKind, FrameResult, Sink};
use crate::protocol::frame::Decoder;

#[allow(clippy::unusual_byte_groupings)] // UUID groups read best in their canonical form
pub const STUDIO_SERVICE_UUID: Uuid = Uuid::from_u128(0x00000000_0196_6107_c967_c5cfb1c2482a);
#[allow(clippy::unusual_byte_groupings)]
pub const STUDIO_RPC_CHARACTERISTIC_UUID: Uuid =
    Uuid::from_u128(0x00000001_0196_6107_c967_c5cfb1c2482a);

const ADAPTER_TIMEOUT: Duration = Duration::from_secs(4);
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_WRITE_CHUNK: usize = 180;

fn debug(message: impl AsRef<str>) {
    if std::env::var_os("ZMK_TUI_BLE_DEBUG").is_some() {
        eprintln!("[ble] {}", message.as_ref());
    }
}

#[derive(Debug, Clone)]
pub struct BleDevice {
    /// The same adapter that discovered the device. bluest's CoreBluetooth
    /// backend creates a separate central manager per adapter, so device
    /// handles must be used with the adapter that produced them.
    pub adapter: Adapter,
    pub device: Device,
    /// Read while the scan adapter is alive. CoreBluetooth peripheral handles
    /// must not be messaged after their adapter is dropped.
    pub id: DeviceId,
    pub name: String,
    pub connected: bool,
}

impl BleDevice {
    pub fn label(&self) -> String {
        if self.connected {
            format!("{}  (connected)", self.name)
        } else {
            self.name.clone()
        }
    }
}

async fn adapter() -> Result<Adapter> {
    let adapter = Adapter::default()
        .await
        .ok_or_else(|| anyhow!("no Bluetooth adapter found"))?;
    tokio::time::timeout(ADAPTER_TIMEOUT, adapter.wait_available())
        .await
        .map_err(|_| anyhow!("the Bluetooth adapter is unavailable (is Bluetooth turned on?)"))?
        .context("the Bluetooth adapter is unavailable")?;
    Ok(adapter)
}

/// Look for keyboards exposing the Studio service.
///
/// Devices already connected to this computer are returned first. Advertising
/// devices are also accepted if a firmware build advertises the service UUID.
pub async fn scan(duration: Duration) -> Result<Vec<BleDevice>> {
    let adapter = adapter().await?;
    let mut discovered = adapter
        .discover_devices(&[STUDIO_SERVICE_UUID])
        .await
        .context("could not start a Bluetooth scan")?;

    let deadline = tokio::time::Instant::now() + duration;
    let mut found: Vec<BleDevice> = Vec::new();
    let mut last_error: Option<String> = None;

    loop {
        let next = tokio::time::timeout_at(deadline, discovered.next()).await;
        match next {
            Err(_) | Ok(None) => break,
            Ok(Some(Err(error))) => last_error = Some(error.to_string()),
            Ok(Some(Ok(device))) => {
                let id = device.id();
                if found.iter().any(|existing| existing.id == id) {
                    continue;
                }
                let name = device
                    .name_async()
                    .await
                    .unwrap_or_else(|_| "Unnamed keyboard".to_string());
                let connected = device.is_connected().await;
                found.push(BleDevice {
                    adapter: adapter.clone(),
                    device,
                    id,
                    name,
                    connected,
                });
            }
        }
    }

    if found.is_empty() {
        if let Some(error) = last_error {
            bail!("Bluetooth scan failed: {error}");
        }
    }

    found.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(found)
}

/// Connect to a discovered keyboard and subscribe to its RPC indications.
pub async fn connect(discovered: BleDevice) -> Result<Connection> {
    let BleDevice {
        id: device_id,
        name,
        ..
    } = discovered;

    // The official Studio client opens a fresh adapter for the real GATT
    // session rather than reusing the scan adapter/handle. Reusing the scan
    // adapter can leave the notification delegate in a state where writes
    // succeed but indications are never delivered on macOS.
    let adapter = Adapter::default()
        .await
        .context("Bluetooth is not available")?;
    tokio::time::timeout(ADAPTER_TIMEOUT, adapter.wait_available())
        .await
        .map_err(|_| anyhow!("the Bluetooth adapter is unavailable (is Bluetooth turned on?)"))?
        .context("Bluetooth is not available")?;
    let device = adapter
        .open_device(&device_id)
        .await
        .with_context(|| format!("could not open {name}"))?;

    // Always ask CoreBluetooth to attach this central, even when the OS
    // reports the keyboard as HID-connected. Skipping this can leave the
    // Studio GATT characteristic writable but indications routed nowhere.
    debug(format!("ensuring connection to {name}"));
    match tokio::time::timeout(Duration::from_secs(20), adapter.connect_device(&device)).await {
        Ok(Ok(())) => debug("connect_device ok"),
        Ok(Err(error)) => {
            if device.is_connected().await {
                debug(format!(
                    "connect_device reported an error while connected: {error}"
                ));
            } else {
                return Err(anyhow!("could not connect to {name}: {error}"));
            }
        }
        Err(_) => return Err(anyhow!("timed out connecting to {name}")),
    }

    let session = match establish_session(&device, &name).await {
        Ok(session) => session,
        Err(error) => {
            // Leaving a half-set-up GATT session behind poisons the next
            // connection attempt, so release the keyboard on every failure
            // path after `connect_device`.
            let _ =
                tokio::time::timeout(Duration::from_secs(2), adapter.disconnect_device(&device))
                    .await;
            return Err(error);
        }
    };

    Ok(Connection {
        label: name,
        kind: ConnectionKind::Bluetooth,
        sink: Sink::Bluetooth(BleSink {
            adapter,
            device,
            characteristic: session.characteristic,
            chunk_size: session.chunk_size,
            notify_task: session.notify_task,
        }),
        frames: session.frames,
    })
}

struct Session {
    characteristic: Characteristic,
    chunk_size: usize,
    frames: tokio::sync::mpsc::UnboundedReceiver<FrameResult>,
    notify_task: tokio::task::JoinHandle<()>,
}

async fn establish_session(device: &Device, name: &str) -> Result<Session> {
    let service = device
        .discover_services_with_uuid(STUDIO_SERVICE_UUID)
        .await
        .with_context(|| format!("could not discover services on {name}"))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            anyhow!(
                "{name} does not expose the ZMK Studio GATT service. \
                 Check that the firmware was built with CONFIG_ZMK_STUDIO=y."
            )
        })?;

    let characteristic = service
        .discover_characteristics_with_uuid(STUDIO_RPC_CHARACTERISTIC_UUID)
        .await
        .with_context(|| format!("could not discover the Studio RPC characteristic on {name}"))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("{name} exposes the Studio service but no RPC characteristic"))?;

    let chunk_size = characteristic
        .max_write_len_async()
        .await
        .unwrap_or(20)
        .clamp(20, MAX_WRITE_CHUNK);

    let (frames_tx, frames) = tokio::sync::mpsc::unbounded_channel::<FrameResult>();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

    let notifier = characteristic.clone();

    let notify_task = tokio::spawn(async move {
        let stream = match notifier.notify().await {
            Ok(stream) => {
                debug("subscribed to indications");
                let _ = ready_tx.send(Ok(()));
                stream
            }
            Err(error) => {
                let message = format!("could not subscribe to Bluetooth updates: {error}");
                let _ = ready_tx.send(Err(message.clone()));
                let _ = frames_tx.send(Err(message));
                return;
            }
        };
        futures::pin_mut!(stream);
        let mut decoder = Decoder::new();
        let mut completed = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(bytes) => {
                    debug(format!("rx {} bytes", bytes.len()));
                    completed.clear();
                    let result = decoder.feed_slice(&bytes, &mut completed);
                    // Keep frames that completed before a corrupt byte run.
                    for frame in completed.drain(..) {
                        if frames_tx.send(Ok(frame)).is_err() {
                            return;
                        }
                    }
                    if result.is_err() {
                        continue;
                    }
                }
                Err(error) => {
                    debug(format!("notification error: {error}"));
                    let _ = frames_tx.send(Err(format!("Bluetooth read failed: {error}")));
                    return;
                }
            }
        }
        let _ = frames_tx.send(Err("Bluetooth connection closed".to_string()));
    });

    let ready = tokio::time::timeout(SUBSCRIBE_TIMEOUT, ready_rx).await;
    match ready {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            notify_task.abort();
            return Err(anyhow!(error));
        }
        Ok(Err(_)) => {
            notify_task.abort();
            return Err(anyhow!("Bluetooth subscription task stopped unexpectedly"));
        }
        Err(_) => {
            notify_task.abort();
            return Err(anyhow!(
                "timed out subscribing to Bluetooth updates on {name}"
            ));
        }
    }

    // CoreBluetooth can report the CCC write as complete a moment before
    // indications are actually flowing. Give the first request a small head
    // start; without this, a quick reconnect can lose its first response.
    tokio::time::sleep(Duration::from_millis(150)).await;

    if std::env::var_os("ZMK_TUI_BLE_DEBUG").is_some() {
        if let Ok(descriptors) = characteristic.discover_descriptors().await {
            for descriptor in descriptors {
                if descriptor.uuid() == Uuid::from_u128(0x00002902_0000_1000_8000_00805f9b34fb) {
                    match descriptor.read().await {
                        Ok(value) => debug(format!("CCC value: {value:?}")),
                        Err(error) => debug(format!("could not read CCC: {error}")),
                    }
                }
            }
        }
    }

    Ok(Session {
        characteristic,
        chunk_size,
        frames,
        notify_task,
    })
}

pub struct BleSink {
    adapter: Adapter,
    device: Device,
    characteristic: Characteristic,
    chunk_size: usize,
    notify_task: tokio::task::JoinHandle<()>,
}

impl Drop for BleSink {
    fn drop(&mut self) {
        // Without this, an idle notified characteristic can keep the GATT
        // connection/subscription alive after the UI returns to the connect
        // screen, making the next connection lose its first response.
        self.notify_task.abort();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let adapter = self.adapter.clone();
        let device = self.device.clone();
        handle.spawn(async move {
            let _ =
                tokio::time::timeout(Duration::from_secs(2), adapter.disconnect_device(&device))
                    .await;
        });
    }
}

impl BleSink {
    pub async fn send(&self, framed: Vec<u8>) -> anyhow::Result<()> {
        debug(format!("tx {} bytes", framed.len()));
        let write = async {
            for chunk in framed.chunks(self.chunk_size) {
                self.characteristic
                    .write(chunk)
                    .await
                    .map_err(|error| anyhow!("Bluetooth write failed: {error}"))?;
            }
            Ok::<(), anyhow::Error>(())
        };
        tokio::time::timeout(WRITE_TIMEOUT, write)
            .await
            .map_err(|_| anyhow!("Bluetooth write timed out"))?
    }
}

/// Terse descriptions help the connect screen explain failures.
pub fn troubleshooting_hint() -> &'static str {
    "Bluetooth needs the keyboard to be connected to this computer, and the \
     firmware must be built with CONFIG_ZMK_STUDIO=y."
}
