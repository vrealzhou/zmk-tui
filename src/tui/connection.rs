//! Connection establishment and the snapshot of device data used by the UI.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::client::{StudioClient, StudioError};
use crate::model::{Catalog, DeviceInfo, Keymap, LockState, PhysicalLayouts};
use crate::transport::{Connection, ConnectionKind, ble, serial};
use crate::tui::app::Msg;

#[derive(Default)]
pub struct Snapshot {
    pub keymap: Keymap,
    pub layouts: PhysicalLayouts,
    pub catalog: Catalog,
    pub dirty: bool,
    pub warnings: Vec<String>,
}

pub struct Connected {
    pub client: StudioClient,
    pub label: String,
    pub kind: ConnectionKind,
    pub info: DeviceInfo,
    pub lock: LockState,
    pub snapshot: Option<Snapshot>,
}

pub enum DeviceChoice {
    Usb { path: String, label: String },
    Bluetooth { device: ble::BleDevice },
}

impl DeviceChoice {
    pub fn label(&self) -> String {
        match self {
            Self::Usb { label, .. } => label.clone(),
            Self::Bluetooth { device } => device.name.clone(),
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
    let client = StudioClient::start(connection);

    // Forward lock/unsaved/disconnect notifications to the UI.
    let mut subscriptions = client.subscribe();
    let event_sender = events.clone();
    tokio::spawn(async move {
        while let Ok(event) = subscriptions.recv().await {
            if event_sender.send(Msg::DeviceEvent(event)).is_err() {
                break;
            }
        }
    });

    let info = client
        .get_device_info()
        .await
        .map_err(|error| friendly(&error))?;
    let mut lock = client
        .get_lock_state()
        .await
        .map_err(|error| friendly(&error))?;

    let mut snapshot = None;
    if lock == LockState::Unlocked {
        match load_snapshot(&client).await {
            Ok(loaded) => snapshot = Some(loaded),
            Err(StudioError::UnlockRequired) => lock = LockState::Locked,
            Err(error) => return Err(friendly(&error)),
        }
    }

    Ok(Connected {
        client,
        label,
        kind,
        info,
        lock,
        snapshot,
    })
}

pub async fn load_snapshot(client: &StudioClient) -> Result<Snapshot, StudioError> {
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
