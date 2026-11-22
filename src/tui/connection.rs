//! Connection establishment and the snapshot of device data used by the UI.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::UnboundedSender;

use crate::client::{StudioClient, StudioError};
use crate::device::DeviceClient;
use crate::model::{Catalog, DeviceInfo, Keymap, LockState, PhysicalLayouts};
use crate::transport::{Connection, ConnectionKind, ble, serial};
use crate::tui::app::{Msg, SessionMsg, VialStart};
use crate::vial::RawHid;
use crate::vial::client::VialClient;
use crate::vial::simulate::SimulatedVial;
use crate::vial::transport::{VialDeviceInfo, list_vial_devices};

/// Every completed connection gets a unique id. The UI uses it to discard
/// completions and notifications that belong to a keyboard it has left.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn next_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub keymap: Keymap,
    pub layouts: PhysicalLayouts,
    pub catalog: Catalog,
    pub dirty: bool,
    pub warnings: Vec<String>,
}

pub struct Connected {
    pub session_id: u64,
    pub client: DeviceClient,
    pub label: String,
    pub kind: ConnectionKind,
    pub info: DeviceInfo,
    pub lock: LockState,
    pub snapshot: Option<Snapshot>,
    /// Set when the handshake succeeded but the initial keymap read failed in a
    /// way worth retrying. The editor opens and retries instead of dropping the
    /// user back to the connect screen.
    pub snapshot_error: Option<String>,
}

/// One row in the connect screen's USB/HID list. The transport is also the
/// standard: a CDC-ACM serial port is a ZMK Studio candidate, a raw-HID
/// interface with Vial's serial magic is a Vial keyboard.
#[derive(Clone, Debug)]
pub enum UsbEntry {
    Studio(serial::UsbDevice),
    Vial(VialDeviceInfo),
}

impl UsbEntry {
    pub fn label(&self) -> String {
        match self {
            Self::Studio(device) => match device.product.as_deref() {
                Some(product) if !product.is_empty() => {
                    format!("{product} (ZMK Studio)  {}", device.path)
                }
                _ => format!("{} (ZMK Studio)", device.path),
            },
            Self::Vial(device) => device.label(),
        }
    }

    pub fn standard(&self) -> &'static str {
        match self {
            Self::Studio(_) => "ZMK Studio",
            Self::Vial(_) => "Vial",
        }
    }

    /// The backend choice for this row. A keyboard that exposes both a Studio
    /// CDC port and a Vial raw-HID interface appears as two rows; selecting
    /// either switches to that standard without restarting.
    pub fn choice(&self) -> DeviceChoice {
        match self {
            Self::Studio(device) => DeviceChoice::Usb {
                path: device.path.clone(),
                label: self.label(),
            },
            Self::Vial(device) => DeviceChoice::Vial {
                path: device.path.clone(),
                label: device.label(),
            },
        }
    }
}

/// Every USB/HID keyboard visible right now, already classified by interface.
pub fn list_usb_entries() -> Vec<UsbEntry> {
    let mut entries: Vec<UsbEntry> = serial::list_usb_devices()
        .into_iter()
        .map(UsbEntry::Studio)
        .collect();
    entries.extend(list_vial_devices().into_iter().map(UsbEntry::Vial));
    entries
}

pub enum DeviceChoice {
    Usb { path: String, label: String },
    Vial { path: String, label: String },
    Bluetooth { device: ble::BleDevice },
}

impl DeviceChoice {
    pub fn label(&self) -> String {
        match self {
            Self::Usb { label, .. } | Self::Vial { label, .. } => label.clone(),
            Self::Bluetooth { device } => device.name.clone(),
        }
    }

    pub fn standard(&self) -> &'static str {
        match self {
            Self::Usb { .. } => "ZMK Studio",
            Self::Vial { .. } => "Vial",
            Self::Bluetooth { .. } => "ZMK Studio",
        }
    }
}

pub async fn scan_bluetooth() -> Result<Vec<ble::BleDevice>, String> {
    ble::scan(Duration::from_secs(3))
        .await
        .map_err(|error| format!("{error:#}"))
}

pub async fn connect_choice(
    choice: DeviceChoice,
    events: UnboundedSender<Msg>,
) -> Result<Connected, String> {
    let connection = match choice {
        DeviceChoice::Usb { path, .. } => {
            let path_for_task = path.clone();
            tokio::task::spawn_blocking(move || serial::connect(&path_for_task))
                .await
                .map_err(|error| format!("USB connection task failed: {error}"))?
                .map_err(|error| format!("{error:#}"))?
        }
        DeviceChoice::Bluetooth { device } => ble::connect(device)
            .await
            .map_err(|error| format!("{error:#}"))?,
        DeviceChoice::Vial { path, .. } => {
            let transport =
                crate::vial::transport::open_path(&path).map_err(|error| error.to_string())?;
            return finish_vial_connection(Box::new(transport), ConnectionKind::Vial).await;
        }
    };
    finish_connection(connection, events).await
}

/// Connect on the first Bluetooth keyboard whose name contains `query`.
pub async fn connect_by_bluetooth_name(
    query: &str,
    events: UnboundedSender<Msg>,
) -> Result<Connected, String> {
    let devices = scan_bluetooth().await?;
    if devices.is_empty() {
        return Err(
            "No ZMK Studio keyboard found over Bluetooth. Make sure the keyboard is \
             connected to this computer and the firmware enables Studio."
                .into(),
        );
    }
    let query_lower = query.to_lowercase();
    let mut matches: Vec<ble::BleDevice> = devices
        .into_iter()
        .filter(|device| device.name.to_lowercase().contains(&query_lower))
        .collect();
    if matches.is_empty() {
        return Err(format!(
            "No Bluetooth keyboard matching “{query}” was found."
        ));
    }
    if matches.len() > 1 {
        return Err(format!(
            "Several Bluetooth keyboards match “{query}”; pass a more specific name."
        ));
    }
    let device = matches.remove(0);
    connect_choice(DeviceChoice::Bluetooth { device }, events).await
}

/// Complete the ZMK Studio handshake and load the initial UI data.
pub async fn finish_connection(
    connection: Connection,
    events: UnboundedSender<Msg>,
) -> Result<Connected, String> {
    let label = connection.label.clone();
    let kind = connection.kind;
    let session_id = next_session_id();
    let client = DeviceClient::Studio(StudioClient::start(connection));

    // Forward lock/unsaved/disconnect notifications to the UI. Vial has no
    // notification channel, so there is nothing to spawn for it. A lagged
    // receiver only skipped notifications; keep forwarding newer ones.
    if let Some(mut subscriptions) = client.subscribe() {
        let event_sender = events.clone();
        tokio::spawn(async move {
            loop {
                match subscriptions.recv().await {
                    Ok(event) => {
                        let message = Msg::Session {
                            session_id,
                            event: SessionMsg::DeviceEvent(event),
                        };
                        if event_sender.send(message).is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    let info = client
        .get_device_info()
        .await
        .map_err(|error| friendly(&error))?;
    let mut lock = client
        .get_lock_state()
        .await
        .map_err(|error| friendly(&error))?;

    let mut snapshot = None;
    let mut snapshot_error = None;
    if lock == LockState::Unlocked {
        match load_snapshot(&client).await {
            Ok(loaded) => snapshot = Some(loaded),
            Err(StudioError::UnlockRequired) => lock = LockState::Locked,
            // A dead transport is a failed connection; a missing RPC means the
            // firmware cannot provide a keymap at all. Everything else (usually
            // a timeout) is worth retrying from the editor.
            Err(error) if error.is_disconnect() => return Err(friendly(&error)),
            Err(error @ StudioError::RpcNotFound) => return Err(friendly(&error)),
            Err(error) => snapshot_error = Some(error.to_string()),
        }
    }

    Ok(Connected {
        session_id,
        client,
        label,
        kind,
        info,
        lock,
        snapshot,
        snapshot_error,
    })
}

/// Connect to a Vial keyboard, either the in-process demo or the first USB
/// raw-HID interface. The handshake and keymap read happen before the UI
/// switches to the editor, so protocol errors are shown on the connect screen.
pub async fn connect_vial(start: VialStart) -> Result<Connected, String> {
    match start {
        VialStart::Simulate => {
            finish_vial_connection(
                Box::new(SimulatedVial::demo()),
                ConnectionKind::VialSimulator,
            )
            .await
        }
        VialStart::Usb => {
            let transport =
                crate::vial::transport::VialHid::open_first().map_err(|error| error.to_string())?;
            finish_vial_connection(Box::new(transport), ConnectionKind::Vial).await
        }
    }
}

async fn finish_vial_connection(
    transport: Box<dyn RawHid>,
    kind: ConnectionKind,
) -> Result<Connected, String> {
    let client = VialClient::open(transport)
        .await
        .map_err(|error| friendly(&error))?;
    let device = DeviceClient::Vial(client);
    let info = device
        .get_device_info()
        .await
        .map_err(|error| friendly(&error))?;
    let snapshot = load_snapshot(&device)
        .await
        .map_err(|error| friendly(&error))?;
    Ok(Connected {
        session_id: next_session_id(),
        client: device,
        label: format!("{} (Vial)", info.name),
        kind,
        info,
        lock: LockState::Unlocked,
        snapshot: Some(snapshot),
        snapshot_error: None,
    })
}

pub async fn load_snapshot(client: &DeviceClient) -> Result<Snapshot, StudioError> {
    let keymap = client.get_keymap().await?;
    // Older/limited firmware may not implement physical layouts. Keep editing
    // available with the generic fallback grid instead of failing the whole
    // connection.
    let layouts = match client.get_physical_layouts().await {
        Ok(layouts) => layouts,
        Err(StudioError::RpcNotFound) => PhysicalLayouts::default(),
        Err(error) => return Err(error),
    };
    let (catalog, warnings) = client.load_catalog().await?;
    let dirty = client.check_unsaved_changes().await?;
    Ok(Snapshot {
        keymap,
        layouts,
        catalog,
        dirty,
        warnings,
    })
}

fn friendly(error: &StudioError) -> String {
    match error {
        StudioError::UnlockRequired => "the keyboard is locked".into(),
        StudioError::RpcNotFound => {
            "the keyboard does not support this request; check that Studio is enabled".into()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_errors_name_lock_and_rpc_support() {
        assert_eq!(
            friendly(&StudioError::UnlockRequired),
            "the keyboard is locked"
        );
        assert!(friendly(&StudioError::RpcNotFound).contains("does not support"));
        assert_eq!(
            friendly(&StudioError::Timeout),
            StudioError::Timeout.to_string()
        );
    }
}
