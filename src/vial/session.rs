//! Editor-facing session for a Vial keyboard.
//!
//! The session projects the Vial device onto the same domain model the ZMK
//! Studio UI already consumes (`Keymap`, `PhysicalLayouts`, `Catalog`,
//! `Binding`). That is the seam that keeps a future TUI integration small: the
//! UI does not need to know whether a binding is a ZMK behavior or a QMK
//! keycode.
//!
//! ## Write model
//!
//! Unlike Studio, Vial writes each keycode straight to EEPROM. To preserve the
//! editor's "edit live, save explicitly, discard reloads" contract, this
//! session stages edits in memory and only sends `set_keycode` on
//! [`VialKeyboard::save`]. Closing the editor without saving therefore leaves
//! the keyboard untouched, and a failed bulk write leaves the failed entries
//! staged for retry. This is deliberately safer than Vial GUI's immediate
//! writes, at the cost of the keyboard not reflecting edits until save.

use std::collections::BTreeMap;

use crate::model::{Binding, Catalog, DeviceInfo, Keymap, Layer, LockState, PhysicalLayouts};
use crate::vial::definition::KeyboardDefinition;
use crate::vial::keycode::{self, Dialect, Translator};
use crate::vial::protocol::{ProtocolInfo, RawHid, Result, UnlockStatus, VialDevice, VialError};

/// What the Vial backend can and cannot do, so a UI can hide unsupported
/// actions instead of offering buttons that fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Dynamic keymap layers are fixed by `DYNAMIC_KEYMAP_LAYER_COUNT`.
    pub add_layers: bool,
    pub remove_layers: bool,
    pub rename_layers: bool,
    pub move_layers: bool,
    /// Physical geometry exists, but there is only the layout baked into the
    /// definition (no active-layout switching).
    pub switch_physical_layouts: bool,
    /// Vial's unlock gesture requires holding matrix keys; the host can poll it.
    pub unlock: bool,
    /// Edits are staged and persisted by an explicit save.
    pub staged_writes: bool,
}

pub const CAPABILITIES: Capabilities = Capabilities {
    add_layers: false,
    remove_layers: false,
    rename_layers: false,
    move_layers: false,
    switch_physical_layouts: false,
    unlock: true,
    staged_writes: true,
};

/// Everything the UI needs after a successful connection.
#[derive(Debug, Clone)]
pub struct VialSnapshot {
    pub keymap: Keymap,
    pub layouts: PhysicalLayouts,
    pub catalog: Catalog,
    pub warnings: Vec<String>,
}

pub struct VialKeyboard<T: RawHid> {
    device: VialDevice<T>,
    info: ProtocolInfo,
    definition: KeyboardDefinition,
    translator: Translator,
    /// Raw keycodes indexed by `[layer][physical key]`, aligned with
    /// `definition.keys`.
    raw: Vec<Vec<u16>>,
    /// Keycodes changed locally but not yet written to the keyboard.
    pending: BTreeMap<(u8, usize), u16>,
    unlocked: bool,
}

impl<T: RawHid> VialKeyboard<T> {
    /// Handshake, fetch the embedded definition, then read the whole keymap.
    pub fn open(transport: T) -> Result<Self> {
        let mut device = VialDevice::new(transport);
        let info = device.handshake()?;
        let definition = KeyboardDefinition::parse(&device.definition_json()?)?;
        let translator = Translator::new(Dialect::from_vial_protocol(info.vial.protocol));
        let mut keyboard = Self {
            device,
            info,
            definition,
            translator,
            raw: Vec::new(),
            pending: BTreeMap::new(),
            unlocked: false,
        };
        keyboard.reload()?;
        keyboard.unlocked = keyboard
            .device
            .unlock_status()
            .map(|status| status.unlocked)
            .unwrap_or(false);
        Ok(keyboard)
    }

    pub fn info(&self) -> ProtocolInfo {
        self.info
    }

    pub fn transport(&self) -> &T {
        self.device.transport()
    }

    pub fn transport_mut(&mut self) -> &mut T {
        self.device.transport_mut()
    }

    pub fn definition(&self) -> &KeyboardDefinition {
        &self.definition
    }

    pub fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    pub fn layer_count(&self) -> usize {
        self.raw.len()
    }

    pub fn matrix(&self) -> crate::vial::definition::Matrix {
        self.definition.matrix
    }

    pub fn uid(&self) -> [u8; 8] {
        self.info.vial.uid
    }

    pub fn raw_keycodes(&self) -> &[Vec<u16>] {
        &self.raw
    }

    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            name: self.definition.name.clone(),
            serial_number: self.info.vial.uid.to_vec(),
        }
    }

    pub fn lock_state(&self) -> LockState {
        if self.unlocked {
            LockState::Unlocked
        } else {
            LockState::Locked
        }
    }

    pub fn has_unsaved_changes(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn snapshot(&self) -> VialSnapshot {
        let unsupported = self
            .raw
            .iter()
            .flat_map(|layer| layer.iter())
            .filter(|keycode| {
                self.translator.decode(**keycode).behavior_id == keycode::behavior::RAW
            })
            .count();
        let mut warnings = Vec::new();
        if self.definition.duplicates > 0 {
            warnings.push(format!(
                "{} key position(s) share a matrix cell (layout options); only the first is editable",
                self.definition.duplicates
            ));
        }
        if self.definition.encoders > 0 {
            warnings.push(format!(
                "{} encoder(s) are not exposed by this editor",
                self.definition.encoders
            ));
        }
        if unsupported > 0 {
            warnings.push(format!(
                "{unsupported} keycode(s) are shown as raw Vial values and cannot be edited structurally"
            ));
        }
        VialSnapshot {
            keymap: self.keymap(),
            layouts: self.layouts(),
            catalog: keycode::catalog(),
            warnings,
        }
    }

    pub fn keymap(&self) -> Keymap {
        Keymap {
            layers: self
                .raw
                .iter()
                .enumerate()
                .map(|(index, keycodes)| Layer {
                    id: index as u32,
                    name: String::new(),
                    bindings: keycodes
                        .iter()
                        .map(|keycode| self.translator.decode(*keycode))
                        .collect(),
                })
                .collect(),
            // Every layer is occupied and Vial cannot create more at runtime.
            available_layers: 0,
            max_layer_name_length: 0,
        }
    }

    pub fn layouts(&self) -> PhysicalLayouts {
        PhysicalLayouts {
            active_index: 0,
            layouts: vec![self.definition.physical_layout()],
        }
    }

    pub fn catalog(&self) -> Catalog {
        keycode::catalog()
    }

    /// QMK keycode at a physical position, including staged edits.
    pub fn keycode_at(&self, layer: usize, position: usize) -> Option<u16> {
        self.raw.get(layer)?.get(position).copied()
    }

    /// Stage a binding change. Returns an error when the chosen binding cannot
    /// be represented as a Vial keycode (for example a consumer keycode inside
    /// a layer-tap).
    pub fn set_binding(&mut self, layer_id: u32, position: usize, binding: Binding) -> Result<()> {
        let keycode = self.translator.encode(binding).ok_or_else(|| {
            VialError::Keycode(format!(
                "{} cannot be represented as a Vial keycode",
                self.translator.label(binding)
            ))
        })?;
        self.stage_keycode(layer_id, position, keycode)
    }

    /// Stage a raw QMK keycode, bypassing the binding translator. Used by the
    /// `.vil` restore path to preserve keycodes the catalog cannot express.
    pub fn stage_keycode(&mut self, layer_id: u32, position: usize, keycode: u16) -> Result<()> {
        let layer = usize::try_from(layer_id)
            .ok()
            .filter(|layer| *layer < self.raw.len())
            .ok_or_else(|| VialError::Keycode(format!("no layer with id {layer_id}")))?;
        if position >= self.definition.keys.len() {
            return Err(VialError::Keycode(format!(
                "no key position {position} (keyboard has {})",
                self.definition.keys.len()
            )));
        }
        self.raw[layer][position] = keycode;
        self.pending.insert((layer as u8, position), keycode);
        Ok(())
    }

    /// Write every staged keycode. Entries that fail stay staged so a retry
    /// only repeats the failed writes.
    pub fn save(&mut self) -> Result<()> {
        let writes: Vec<((u8, usize), u16)> = self
            .pending
            .iter()
            .map(|(position, keycode)| (*position, *keycode))
            .collect();
        let mut failed = BTreeMap::new();
        let mut first_error = None;
        for ((layer, position), keycode) in writes {
            let key = self.definition.keys[position];
            match self.device.set_keycode(layer, key.row, key.col, keycode) {
                Ok(()) => {}
                Err(error) => {
                    failed.insert((layer, position), keycode);
                    first_error.get_or_insert(error);
                }
            }
        }
        self.pending = failed;
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Drop staged edits and re-read the keyboard.
    pub fn discard(&mut self) -> Result<()> {
        self.pending.clear();
        self.reload()
    }

    /// Restore the compiled keymap (`id_dynamic_keymap_reset`). Macros, tap
    /// dance and combos are separate Vial settings and are not touched.
    pub fn reset_keymap(&mut self) -> Result<()> {
        self.pending.clear();
        self.device.reset_keymap()?;
        self.reload()
    }

    pub fn unlock_status(&mut self) -> Result<UnlockStatus> {
        let status = self.device.unlock_status()?;
        self.unlocked = status.unlocked;
        Ok(status)
    }

    pub fn unlock_start(&mut self) -> Result<()> {
        self.device.unlock_start()
    }

    pub fn unlock_poll(&mut self) -> Result<UnlockStatus> {
        let status = self.device.unlock_poll()?;
        self.unlocked = status.unlocked;
        Ok(status)
    }

    pub fn lock(&mut self) -> Result<()> {
        self.device.lock()?;
        self.unlocked = false;
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let layers = self.device.layer_count()?;
        let matrix = self.definition.matrix;
        let buffer = self.device.keymap(layers, matrix.rows, matrix.cols)?;
        let mut raw = vec![vec![0u16; self.definition.keys.len()]; usize::from(layers)];
        for (layer, keycodes) in raw.iter_mut().enumerate() {
            for (index, key) in self.definition.keys.iter().enumerate() {
                let cell = layer * matrix.cell_count()
                    + usize::from(key.row) * usize::from(matrix.cols)
                    + usize::from(key.col);
                keycodes[index] = buffer[cell];
            }
        }
        self.raw = raw;
        Ok(())
    }
}
