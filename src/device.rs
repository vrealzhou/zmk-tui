//! Device backend abstraction.
//!
//! The UI was written against [`StudioClient`]. A Vial keyboard speaks a
//! completely different protocol, so this enum gives the UI one client type
//! with the same method signatures. Both backends surface failures as
//! [`StudioError`], which keeps the existing modal/status handling and message
//! types untouched.
//!
//! Capabilities tell the UI which global actions exist. Vial has a fixed number
//! of dynamic-keymap layers and no Studio-style physical-layout switching, so
//! those dialogs are hidden instead of being offered and failing.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::client::{StudioClient, StudioError, StudioEvent};
use crate::model::{Binding, Catalog, DeviceInfo, Keymap, LockState, PhysicalLayouts};

use crate::vial::client::VialClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub manage_layers: bool,
    pub switch_physical_layouts: bool,
}

impl Capabilities {
    pub const STUDIO: Self = Self {
        manage_layers: true,
        switch_physical_layouts: true,
    };
    pub const VIAL: Self = Self {
        manage_layers: false,
        switch_physical_layouts: false,
    };
}

#[derive(Clone)]
pub enum DeviceClient {
    Studio(StudioClient),
    Vial(Arc<VialClient>),
}

impl DeviceClient {
    pub fn capabilities(&self) -> Capabilities {
        match self {
            Self::Studio(_) => Capabilities::STUDIO,
            Self::Vial(_) => Capabilities::VIAL,
        }
    }

    /// Studio forwards lock/unsaved notifications; Vial has no notifications,
    /// so callers get `None` and simply do not spawn a forwarding task.
    pub fn subscribe(&self) -> Option<broadcast::Receiver<StudioEvent>> {
        match self {
            Self::Studio(client) => Some(client.subscribe()),
            Self::Vial(_) => None,
        }
    }

    pub fn try_cached_lock_state(&self) -> Option<Option<LockState>> {
        match self {
            Self::Studio(client) => client.try_cached_lock_state(),
            // Vial dynamic-keymap writes are allowed while the firmware is
            // locked; the editor does not touch macros or the bootloader.
            Self::Vial(_) => Some(Some(LockState::Unlocked)),
        }
    }

    pub async fn get_device_info(&self) -> Result<DeviceInfo, StudioError> {
        match self {
            Self::Studio(client) => client.get_device_info().await,
            Self::Vial(client) => client.get_device_info().await,
        }
    }

    pub async fn get_lock_state(&self) -> Result<LockState, StudioError> {
        match self {
            Self::Studio(client) => client.get_lock_state().await,
            Self::Vial(client) => client.get_lock_state().await,
        }
    }

    pub async fn get_keymap(&self) -> Result<Keymap, StudioError> {
        match self {
            Self::Studio(client) => client.get_keymap().await,
            Self::Vial(client) => client.get_keymap().await,
        }
    }

    pub async fn get_physical_layouts(&self) -> Result<PhysicalLayouts, StudioError> {
        match self {
            Self::Studio(client) => client.get_physical_layouts().await,
            Self::Vial(client) => client.get_physical_layouts().await,
        }
    }

    pub async fn check_unsaved_changes(&self) -> Result<bool, StudioError> {
        match self {
            Self::Studio(client) => client.check_unsaved_changes().await,
            Self::Vial(client) => client.check_unsaved_changes().await,
        }
    }

    pub async fn load_catalog(&self) -> Result<(Catalog, Vec<String>), StudioError> {
        match self {
            Self::Studio(client) => client.load_catalog().await,
            Self::Vial(client) => client.load_catalog().await,
        }
    }

    pub async fn set_layer_binding(
        &self,
        layer_id: u32,
        position: usize,
        binding: Binding,
    ) -> Result<(), StudioError> {
        match self {
            Self::Studio(client) => client.set_layer_binding(layer_id, position, binding).await,
            Self::Vial(client) => client.set_layer_binding(layer_id, position, binding).await,
        }
    }

    pub async fn save_changes(&self) -> Result<(), StudioError> {
        match self {
            Self::Studio(client) => client.save_changes().await,
            Self::Vial(client) => client.save_changes().await,
        }
    }

    pub async fn discard_changes(&self) -> Result<(), StudioError> {
        match self {
            Self::Studio(client) => client.discard_changes().await,
            Self::Vial(client) => client.discard_changes().await,
        }
    }

    pub async fn reset_settings(&self) -> Result<(), StudioError> {
        match self {
            Self::Studio(client) => client.reset_settings().await,
            Self::Vial(client) => client.reset_settings().await,
        }
    }

    pub async fn add_layer(&self) -> Result<u32, StudioError> {
        match self {
            Self::Studio(client) => client.add_layer().await,
            Self::Vial(client) => client.add_layer().await,
        }
    }

    pub async fn remove_layer(&self, index: usize) -> Result<(), StudioError> {
        match self {
            Self::Studio(client) => client.remove_layer(index).await,
            Self::Vial(client) => client.remove_layer(index).await,
        }
    }

    pub async fn move_layer(&self, from: usize, to: usize) -> Result<Keymap, StudioError> {
        match self {
            Self::Studio(client) => client.move_layer(from, to).await,
            Self::Vial(client) => client.move_layer(from, to).await,
        }
    }

    pub async fn set_layer_props(&self, layer_id: u32, name: &str) -> Result<(), StudioError> {
        match self {
            Self::Studio(client) => client.set_layer_props(layer_id, name).await,
            Self::Vial(client) => client.set_layer_props(layer_id, name).await,
        }
    }

    pub async fn set_active_physical_layout(&self, index: usize) -> Result<Keymap, StudioError> {
        match self {
            Self::Studio(client) => client.set_active_physical_layout(index).await,
            Self::Vial(client) => client.set_active_physical_layout(index).await,
        }
    }
}
