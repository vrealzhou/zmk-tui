//! Adapter from [`VialKeyboard`] to the UI's device-client contract.
//!
//! Every failure is translated into a [`StudioError`] variant the UI already
//! understands, which is why adding Vial does not touch the message, modal or
//! status handling. The adapter also answers lock/unsaved queries in the terms
//! the editor uses.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::client::StudioError;
use crate::model::{Binding, Catalog, DeviceInfo, Keymap, LockState, PhysicalLayouts};
use crate::vial::protocol::{RawHid, VialError};
use crate::vial::session::VialKeyboard;

pub struct VialClient {
    keyboard: Mutex<VialKeyboard<Box<dyn RawHid>>>,
    info: DeviceInfo,
}

impl VialClient {
    pub async fn open(transport: Box<dyn RawHid>) -> Result<Arc<Self>, StudioError> {
        let keyboard = VialKeyboard::open(transport).map_err(map_error)?;
        let info = keyboard.device_info();
        Ok(Arc::new(Self {
            keyboard: Mutex::new(keyboard),
            info,
        }))
    }

    pub async fn get_device_info(&self) -> Result<DeviceInfo, StudioError> {
        Ok(self.info.clone())
    }

    /// Vial's lock does not gate the dynamic keymap: Vial GUI edits keycodes
    /// while locked and only runs its unlock gesture before assigning
    /// `QK_BOOT`. The firmware's `vial_keycode_firewall` also refuses to execute
    /// `QK_BOOT` while locked. This editor does not implement the Vial unlock
    /// UI, so reporting `Unlocked` keeps normal keymap editing available and
    /// avoids showing a Studio unlock dialog that cannot unlock a Vial board.
    pub async fn get_lock_state(&self) -> Result<LockState, StudioError> {
        Ok(LockState::Unlocked)
    }

    pub async fn get_keymap(&self) -> Result<Keymap, StudioError> {
        Ok(self.keyboard.lock().await.keymap())
    }

    pub async fn get_physical_layouts(&self) -> Result<PhysicalLayouts, StudioError> {
        Ok(self.keyboard.lock().await.layouts())
    }

    pub async fn check_unsaved_changes(&self) -> Result<bool, StudioError> {
        Ok(self.keyboard.lock().await.has_unsaved_changes())
    }

    pub async fn load_catalog(&self) -> Result<(Catalog, Vec<String>), StudioError> {
        let keyboard = self.keyboard.lock().await;
        Ok((keyboard.catalog(), keyboard.snapshot().warnings))
    }

    pub async fn set_layer_binding(
        &self,
        layer_id: u32,
        position: usize,
        binding: Binding,
    ) -> Result<(), StudioError> {
        self.keyboard
            .lock()
            .await
            .set_binding(layer_id, position, binding)
            .map_err(map_error)
    }

    pub async fn save_changes(&self) -> Result<(), StudioError> {
        self.keyboard.lock().await.save().map_err(map_error)
    }

    pub async fn discard_changes(&self) -> Result<(), StudioError> {
        self.keyboard.lock().await.discard().map_err(map_error)
    }

    /// Vial has no "restore settings" RPC; the closest operation is
    /// `id_dynamic_keymap_reset`, which rewrites only the dynamic keymap from
    /// the compiled one. Macros, tap dance, combos, key overrides, lighting and
    /// QMK settings are left alone.
    pub async fn reset_settings(&self) -> Result<(), StudioError> {
        self.keyboard.lock().await.reset_keymap().map_err(map_error)
    }

    /// Vial's layer count is compiled in; there are no free slots.
    pub async fn add_layer(&self) -> Result<u32, StudioError> {
        Err(StudioError::RpcNotFound)
    }

    pub async fn remove_layer(&self, _index: usize) -> Result<(), StudioError> {
        Err(StudioError::RpcNotFound)
    }

    pub async fn move_layer(&self, _from: usize, _to: usize) -> Result<Keymap, StudioError> {
        Err(StudioError::RpcNotFound)
    }

    pub async fn set_layer_props(&self, _layer_id: u32, _name: &str) -> Result<(), StudioError> {
        Err(StudioError::RpcNotFound)
    }

    pub async fn set_active_physical_layout(&self, index: usize) -> Result<Keymap, StudioError> {
        if index != 0 {
            return Err(StudioError::RpcNotFound);
        }
        self.get_keymap().await
    }

    /// Exposed for the probe and future UI affordances such as a lock badge.
    pub async fn vial_lock_state(&self) -> Result<bool, StudioError> {
        let mut keyboard = self.keyboard.lock().await;
        keyboard
            .unlock_status()
            .map(|status| status.unlocked)
            .map_err(map_error)
    }
}

fn map_error(error: VialError) -> StudioError {
    let message = error.to_string();
    match error {
        VialError::Transport(_) => StudioError::Transport(message),
        VialError::Timeout => StudioError::Timeout,
        VialError::UnsupportedProtocol { .. }
        | VialError::Unexpected(_)
        | VialError::Definition(_) => StudioError::Protocol(message),
        VialError::Keycode(_) => StudioError::Device(message),
        VialError::Unsupported(_) => StudioError::RpcNotFound,
        VialError::Locked => StudioError::UnlockRequired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vial_errors_map_onto_the_ui_error_vocabulary() {
        assert!(matches!(
            map_error(VialError::Transport("x".into())),
            StudioError::Transport(_)
        ));
        assert!(matches!(
            map_error(VialError::Timeout),
            StudioError::Timeout
        ));
        for error in [
            VialError::UnsupportedProtocol { via: 9, vial: 7 },
            VialError::Unexpected("x".into()),
            VialError::Definition("x".into()),
        ] {
            assert!(matches!(map_error(error), StudioError::Protocol(_)));
        }
        assert!(matches!(
            map_error(VialError::Keycode("x".into())),
            StudioError::Device(_)
        ));
        assert!(matches!(
            map_error(VialError::Unsupported("x")),
            StudioError::RpcNotFound
        ));
        assert!(matches!(
            map_error(VialError::Locked),
            StudioError::UnlockRequired
        ));
    }
}
