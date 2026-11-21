//! QMK/Vial keycode translation.
//!
//! A Vial keymap cell is one 16-bit QMK keycode. This editor's domain model is
//! ZMK Studio's `Binding { behavior, param1, param2 }`, so every keycode has to
//! be projected onto a small catalog of pseudo behaviors:
//!
//! | QMK keycode range | pseudo behavior | round-trip |
//! | --- | --- | --- |
//! | `KC_NO` / `KC_TRNS` | None / Transparent | exact |
//! | basic HID usage (`0x04..=0xA4`, `0xE0..=0xE7`) | Key Press | exact |
//! | QMK system/consumer (`0xA5..=0xDF`) | Key Press (remapped to ZMK usage) | exact for the table below |
//! | `QK_MODS` (modifier + key) | Modified Key | exact |
//! | `QK_MOD_TAP` | Mod-Tap (Vial) | exact |
//! | `QK_LAYER_TAP` | Layer-Tap | exact |
//! | `QK_TO` / `QK_MOMENTARY` / `QK_TOGGLE_LAYER` / `QK_ONE_SHOT_LAYER` | layer behaviors | exact |
//! | `QK_ONE_SHOT_MOD` | Sticky Mod | exact |
//! | `QK_BOOTLOADER` / `QK_REBOOT` | Bootloader / Reset | exact |
//! | everything else | Vial Keycode (raw 16-bit) | exact |
//!
//! The raw fallback matters: tap dance, macros, key overrides and custom
//! keycodes are preserved byte-for-byte even though this editor cannot edit
//! their meaning yet. **No 16-bit keycode is ever lost by a read/read-write
//! cycle**, which is the property that makes a third-party editor safe to use.
//!
//! The numeric ranges depend on the Vial protocol version. Protocol 6 (current
//! `vial-qmk`) uses the renumbered QMK keycode space (`QK_MOD_TAP = 0x2000`);
//! protocol 5 and older use the pre-0.24 space (`QK_MOD_TAP = 0x6000`,
//! `QK_TO = 0x5000`). `vial-gui` ships two keycode tables for exactly this
//! reason and selects one from the reported protocol version.

use crate::keycodes;
use crate::model::{Behavior, Binding, Catalog, ParamDesc, ParamKind, ParamSet};

/// Pseudo behavior ids. These never collide with ZMK Studio's CRC-derived ids
/// because they live in a private high range, and the Vial backend is never
/// mixed with a Studio catalog.
pub mod behavior {
    pub const NONE: u32 = 0x5649_0001;
    pub const TRANS: u32 = 0x5649_0002;
    pub const KP: u32 = 0x5649_0003;
    pub const KP_MODS: u32 = 0x5649_0004;
    pub const MT: u32 = 0x5649_0005;
    pub const LT: u32 = 0x5649_0006;
    pub const MO: u32 = 0x5649_0007;
    pub const TO: u32 = 0x5649_0008;
    pub const TOG: u32 = 0x5649_0009;
    pub const SL: u32 = 0x5649_000A;
    pub const SK: u32 = 0x5649_000B;
    pub const BOOTLOADER: u32 = 0x5649_000C;
    pub const SYS_RESET: u32 = 0x5649_000D;
    pub const RAW: u32 = 0x5649_000E;
}

/// QMK keycode numbering family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Vial protocol <= 5: QK_MOD_TAP = 0x6000, QK_TO = 0x5000, ...
    Legacy,
    /// Vial protocol 6+: QK_MOD_TAP = 0x2000, QK_TO = 0x5200, ...
    Modern,
}

impl Dialect {
    pub fn from_vial_protocol(protocol: u32) -> Self {
        if protocol >= super::protocol::VIAL_PROTOCOL_MODERN_KEYCODES {
            Self::Modern
        } else {
            Self::Legacy
        }
    }

    fn mod_tap(self) -> u16 {
        match self {
            Self::Modern => 0x2000,
            Self::Legacy => 0x6000,
        }
    }

    fn to(self) -> u16 {
        match self {
            Self::Modern => 0x5200,
            Self::Legacy => 0x5000,
        }
    }

    fn momentary(self) -> u16 {
        match self {
            Self::Modern => 0x5220,
            Self::Legacy => 0x5100,
        }
    }

    fn toggle_layer(self) -> u16 {
        match self {
            Self::Modern => 0x5260,
            Self::Legacy => 0x5300,
        }
    }

    fn one_shot_layer(self) -> u16 {
        match self {
            Self::Modern => 0x5280,
            Self::Legacy => 0x5400,
        }
    }

    fn one_shot_mod(self) -> u16 {
        match self {
            Self::Modern => 0x52A0,
            Self::Legacy => 0x5500,
        }
    }
}

const QK_BOOTLOADER: u16 = 0x7C00;
const QK_REBOOT: u16 = 0x7C01;
const QK_LAYER_TAP: u16 = 0x4000;
const QK_MACRO_FIRST: u16 = 0x7700;
const QK_MACRO_LAST: u16 = 0x777F;

/// QMK consumer/system keycodes (basic range 0xA5..=0xBE) and the ZMK values
/// they correspond to. QMK uses a compact sequential block; HID consumer page
/// usages are unrelated numbers, so this table is unavoidable.
const CONSUMER: &[(u8, u32)] = &[
    (0xA8, 0x000C_00E2), // C_MUTE
    (0xA9, 0x000C_00E9), // C_VOLUME_UP
    (0xAA, 0x000C_00EA), // C_VOLUME_DOWN
    (0xAB, 0x000C_00B5), // C_NEXT
    (0xAC, 0x000C_00B6), // C_PREVIOUS
    (0xAD, 0x000C_00B7), // C_STOP
    (0xAE, 0x000C_00CD), // C_PLAY_PAUSE
    (0xB0, 0x000C_00B8), // C_EJECT
    (0xB2, 0x000C_0192), // C_AL_CALCULATOR
    (0xBB, 0x000C_00B3), // C_FAST_FORWARD
    (0xBC, 0x000C_00B4), // C_REWIND
    (0xBD, 0x000C_006F), // C_BRIGHTNESS_INC
    (0xBE, 0x000C_0070), // C_BRIGHTNESS_DEC
];

fn consumer_to_zmk(code: u8) -> Option<u32> {
    CONSUMER
        .iter()
        .find_map(|&(qmk, zmk)| (qmk == code).then_some(zmk))
}

fn zmk_to_consumer(value: u32) -> Option<u8> {
    CONSUMER
        .iter()
        .find_map(|&(qmk, zmk)| (zmk == value).then_some(qmk))
}

/// Translate a ZMK `&kp` parameter into a QMK basic keycode. Handles the plain
/// keyboard page, the system keys and the consumer table.
fn zmk_to_basic(value: u32) -> Option<u8> {
    let mods = ((value >> 24) & 0xFF) as u8;
    let page = ((value >> 16) & 0xFF) as u8;
    let usage = (value & 0xFFFF) as u16;
    if mods != 0 {
        return None;
    }
    match page {
        0 | keycodes::KEYBOARD_PAGE => {
            if (0x04..=0xA4).contains(&usage) || (0xE0..=0xE7).contains(&usage) {
                Some(usage as u8)
            } else {
                None
            }
        }
        keycodes::CONSUMER_PAGE => zmk_to_consumer(value),
        _ => None,
    }
}

fn basic_to_zmk(code: u8) -> Option<u32> {
    match code {
        0x04..=0xA4 | 0xE0..=0xE7 => {
            Some(u32::from(keycodes::KEYBOARD_PAGE) << 16 | u32::from(code))
        }
        0xA5..=0xBE => consumer_to_zmk(code),
        _ => None,
    }
}

fn qmk_mods_name(mods: u8) -> String {
    let mut parts = Vec::new();
    let right = mods & 0x10 != 0;
    for (bit, left, right_name) in [
        (0x01, "LCTL", "RCTL"),
        (0x02, "LSFT", "RSFT"),
        (0x04, "LALT", "RALT"),
        (0x08, "LGUI", "RGUI"),
    ] {
        if mods & bit != 0 {
            parts.push(if right { right_name } else { left });
        }
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join("+")
    }
}

/// ZMK modifier byte to a QMK 5-bit modifier mask. Multiple left/right bits
/// collapse onto QMK's single right-hand flag.
fn zmk_mods_to_qmk(mods: u8) -> u8 {
    let mut qmk = 0u8;
    if mods & (0x01 | 0x10) != 0 {
        qmk |= 0x01;
    }
    if mods & (0x02 | 0x20) != 0 {
        qmk |= 0x02;
    }
    if mods & (0x04 | 0x40) != 0 {
        qmk |= 0x04;
    }
    if mods & (0x08 | 0x80) != 0 {
        qmk |= 0x08;
    }
    if mods & 0xF0 != 0 {
        qmk |= 0x10;
    }
    qmk
}

/// Converts between Vial keycodes and the editor's binding model.
#[derive(Debug, Clone, Copy)]
pub struct Translator {
    dialect: Dialect,
}

impl Translator {
    pub fn new(dialect: Dialect) -> Self {
        Self { dialect }
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Decode a 16-bit QMK keycode. Unknown ranges become [`behavior::RAW`],
    /// so decoding is total and always reversible.
    pub fn decode(&self, keycode: u16) -> Binding {
        let binding = |behavior_id, param1, param2| Binding {
            behavior_id,
            param1,
            param2,
        };
        match keycode {
            0x0000 => binding(behavior::NONE, 0, 0),
            0x0001 => binding(behavior::TRANS, 0, 0),
            QK_BOOTLOADER => binding(behavior::BOOTLOADER, 0, 0),
            QK_REBOOT => binding(behavior::SYS_RESET, 0, 0),
            0x0004..=0x00FF => match basic_to_zmk(keycode as u8) {
                Some(zmk) => binding(behavior::KP, zmk, 0),
                None => binding(behavior::RAW, u32::from(keycode), 0),
            },
            0x0100..=0x1FFF => {
                let mods = ((keycode >> 8) & 0x1F) as u8;
                let base = (keycode & 0x00FF) as u8;
                match basic_to_zmk(base) {
                    Some(zmk) => binding(behavior::KP_MODS, u32::from(mods), zmk),
                    None => binding(behavior::RAW, u32::from(keycode), 0),
                }
            }
            // Macro keycodes occupy the same numeric space as mod-tap in the
            // legacy dialect; test them first.
            QK_MACRO_FIRST..=QK_MACRO_LAST => binding(behavior::RAW, u32::from(keycode), 0),
            range if (range & 0xF000) == QK_LAYER_TAP => {
                let layer = u32::from((keycode >> 8) & 0x0F);
                let tap = tap_to_zmk((keycode & 0x00FF) as u8);
                binding(behavior::LT, layer, tap)
            }
            range if self.is_mod_tap(range) => {
                let mods = u32::from((keycode >> 8) & 0x1F);
                let tap = tap_to_zmk((keycode & 0x00FF) as u8);
                binding(behavior::MT, mods, tap)
            }
            range if self.is_to(range) => {
                binding(behavior::TO, u32::from(range - self.dialect.to()), 0)
            }
            range if self.is_momentary(range) => {
                binding(behavior::MO, u32::from(range - self.dialect.momentary()), 0)
            }
            range if self.is_toggle(range) => binding(
                behavior::TOG,
                u32::from(range - self.dialect.toggle_layer()),
                0,
            ),
            range if self.is_one_shot_layer(range) => binding(
                behavior::SL,
                u32::from(range - self.dialect.one_shot_layer()),
                0,
            ),
            range if self.is_one_shot_mod(range) => {
                let mods = u32::from(range - self.dialect.one_shot_mod());
                binding(behavior::SK, mods, 0)
            }
            _ => binding(behavior::RAW, u32::from(keycode), 0),
        }
    }

    fn is_mod_tap(&self, keycode: u16) -> bool {
        match self.dialect {
            Dialect::Modern => (0x2000..=0x3FFF).contains(&keycode),
            Dialect::Legacy => (0x6000..=0x7FFF).contains(&keycode),
        }
    }

    fn is_to(&self, keycode: u16) -> bool {
        let base = self.dialect.to();
        (base..base + 0x20).contains(&keycode)
    }

    fn is_momentary(&self, keycode: u16) -> bool {
        let base = self.dialect.momentary();
        (base..base + 0x20).contains(&keycode)
    }

    fn is_toggle(&self, keycode: u16) -> bool {
        let base = self.dialect.toggle_layer();
        (base..base + 0x20).contains(&keycode)
    }

    fn is_one_shot_layer(&self, keycode: u16) -> bool {
        let base = self.dialect.one_shot_layer();
        (base..base + 0x20).contains(&keycode)
    }

    fn is_one_shot_mod(&self, keycode: u16) -> bool {
        let base = self.dialect.one_shot_mod();
        (base..base + 0x20).contains(&keycode)
    }

    /// Encode a binding back to a QMK keycode. `None` means the binding cannot
    /// be represented (for example a consumer keycode inside a layer-tap).
    pub fn encode(&self, binding: Binding) -> Option<u16> {
        let layer = |value: u32| u16::try_from(value).ok().filter(|value| *value <= 0x1F);
        match binding.behavior_id {
            behavior::NONE => Some(0x0000),
            behavior::TRANS => Some(0x0001),
            behavior::BOOTLOADER => Some(QK_BOOTLOADER),
            behavior::SYS_RESET => Some(QK_REBOOT),
            behavior::KP => {
                let mods = ((binding.param1 >> 24) & 0xFF) as u8;
                let basic = zmk_to_basic(binding.param1)?;
                if mods == 0 {
                    Some(u16::from(basic))
                } else {
                    Some((u16::from(zmk_mods_to_qmk(mods)) << 8) | u16::from(basic))
                }
            }
            behavior::KP_MODS => {
                let basic = zmk_to_basic(binding.param2)?;
                let mods = (binding.param1 as u8) & 0x1F;
                if mods == 0 {
                    Some(u16::from(basic))
                } else {
                    Some((u16::from(mods) << 8) | u16::from(basic))
                }
            }
            behavior::MT => {
                let tap = tap_to_qmk8(binding.param2)?;
                let mods = (binding.param1 as u8) & 0x1F;
                Some(self.dialect.mod_tap() | (u16::from(mods) << 8) | u16::from(tap))
            }
            behavior::LT => {
                let tap = tap_to_qmk8(binding.param2)?;
                let layer = (binding.param1 as u8) & 0x0F;
                Some(QK_LAYER_TAP | (u16::from(layer) << 8) | u16::from(tap))
            }
            behavior::MO => Some(self.dialect.momentary() | layer(binding.param1)?),
            behavior::TO => Some(self.dialect.to() | layer(binding.param1)?),
            behavior::TOG => Some(self.dialect.toggle_layer() | layer(binding.param1)?),
            behavior::SL => Some(self.dialect.one_shot_layer() | layer(binding.param1)?),
            behavior::SK => {
                let mods = (binding.param1 as u8) & 0x1F;
                Some(self.dialect.one_shot_mod() | u16::from(mods))
            }
            behavior::RAW => u16::try_from(binding.param1).ok(),
            _ => None,
        }
    }

    /// Human-readable QMK-style label for a binding.
    pub fn label(&self, binding: Binding) -> String {
        let key = |value: u32| keycodes::name(value);
        match binding.behavior_id {
            behavior::NONE => "KC_NO".into(),
            behavior::TRANS => "KC_TRNS".into(),
            behavior::KP => key(binding.param1),
            behavior::KP_MODS => format!(
                "{}({})",
                qmk_mods_name(binding.param1 as u8),
                key(binding.param2)
            ),
            behavior::MT => format!(
                "MT({}, {})",
                qmk_mods_name(binding.param1 as u8),
                key(binding.param2)
            ),
            behavior::LT => format!("LT({}, {})", binding.param1, key(binding.param2)),
            behavior::MO => format!("MO({})", binding.param1),
            behavior::TO => format!("TO({})", binding.param1),
            behavior::TOG => format!("TG({})", binding.param1),
            behavior::SL => format!("OSL({})", binding.param1),
            behavior::SK => format!("OSM({})", qmk_mods_name(binding.param1 as u8)),
            behavior::BOOTLOADER => "QK_BOOT".into(),
            behavior::SYS_RESET => "QK_RBT".into(),
            behavior::RAW => format!("0x{:04X}", binding.param1 & 0xFFFF),
            _ => format!(
                "behavior 0x{:08X}({}, {})",
                binding.behavior_id, binding.param1, binding.param2
            ),
        }
    }
}

/// Tap keys inside LT/MT are 8-bit QMK keycodes. Store the semantic ZMK value
/// when one exists so the UI can label it, and the raw byte otherwise.
fn tap_to_zmk(tap: u8) -> u32 {
    basic_to_zmk(tap).unwrap_or(u32::from(tap))
}

fn tap_to_qmk8(value: u32) -> Option<u8> {
    if value <= 0xFF {
        return Some(value as u8);
    }
    let mods = ((value >> 24) & 0xFF) as u8;
    if mods != 0 {
        return None;
    }
    zmk_to_basic(value)
}

fn desc(name: &str, kind: ParamKind) -> ParamDesc {
    ParamDesc {
        name: name.into(),
        kind,
    }
}

fn hid() -> ParamKind {
    ParamKind::HidUsage {
        keyboard_max: 0xFFFF,
        consumer_max: 0xFFFF,
    }
}

/// The pseudo behavior catalog advertised to the UI for a Vial keyboard.
pub fn catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let mut add = |id, name: &str, param1: Vec<ParamDesc>, param2: Vec<ParamDesc>| {
        catalog.insert(Behavior {
            id,
            display_name: name.into(),
            metadata: if param1.is_empty() && param2.is_empty() {
                Vec::new()
            } else {
                vec![ParamSet { param1, param2 }]
            },
        });
    };
    add(behavior::NONE, "None", vec![], vec![]);
    add(behavior::TRANS, "Transparent", vec![], vec![]);
    add(behavior::KP, "Key Press", vec![desc("Key", hid())], vec![]);
    add(
        behavior::KP_MODS,
        "Modified Key",
        vec![desc("Modifier mask", ParamKind::Range { min: 0, max: 31 })],
        vec![desc("Key", hid())],
    );
    add(
        behavior::MT,
        "Mod-Tap (Vial)",
        vec![desc("Modifier mask", ParamKind::Range { min: 0, max: 31 })],
        vec![desc("Tap key", hid())],
    );
    add(
        behavior::LT,
        "Layer-Tap",
        vec![desc("Layer", ParamKind::LayerId)],
        vec![desc("Tap key", hid())],
    );
    add(
        behavior::MO,
        "Momentary Layer",
        vec![desc("Layer", ParamKind::LayerId)],
        vec![],
    );
    add(
        behavior::TO,
        "To Layer",
        vec![desc("Layer", ParamKind::LayerId)],
        vec![],
    );
    add(
        behavior::TOG,
        "Toggle Layer",
        vec![desc("Layer", ParamKind::LayerId)],
        vec![],
    );
    add(
        behavior::SL,
        "Sticky Layer",
        vec![desc("Layer", ParamKind::LayerId)],
        vec![],
    );
    add(
        behavior::SK,
        "Sticky Modifier",
        vec![desc("Modifier mask", ParamKind::Range { min: 0, max: 31 })],
        vec![],
    );
    add(behavior::BOOTLOADER, "Bootloader", vec![], vec![]);
    add(behavior::SYS_RESET, "Reset", vec![], vec![]);
    add(
        behavior::RAW,
        "Vial Keycode",
        vec![desc("QMK keycode", ParamKind::Range { min: 0, max: 65535 })],
        vec![],
    );
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translator(protocol: u32) -> Translator {
        Translator::new(Dialect::from_vial_protocol(protocol))
    }

    #[test]
    fn every_16_bit_keycode_round_trips_in_both_dialects() {
        for protocol in [5, 6] {
            let translator = translator(protocol);
            for keycode in 0..=u16::MAX {
                let binding = translator.decode(keycode);
                let encoded = translator
                    .encode(binding)
                    .unwrap_or_else(|| panic!("0x{keycode:04X} encoded to {binding:?}"));
                assert_eq!(
                    encoded, keycode,
                    "0x{keycode:04X} -> {binding:?} -> 0x{encoded:04X}"
                );
            }
        }
    }

    #[test]
    fn layer_tap_and_mod_tap_use_the_dialect_ranges() {
        let modern = translator(6);
        assert_eq!(
            modern.decode(0x4012).behavior_id,
            behavior::LT,
            "LT(0, KC_TAB)"
        );
        assert_eq!(modern.decode(0x2104).behavior_id, behavior::MT);
        assert_eq!(modern.encode(modern.decode(0x2104)), Some(0x2104));

        let legacy = translator(5);
        assert_eq!(legacy.decode(0x6104).behavior_id, behavior::MT);
        assert_eq!(legacy.encode(legacy.decode(0x6104)), Some(0x6104));
        assert_eq!(legacy.decode(0x5101).behavior_id, behavior::MO);
    }

    #[test]
    fn consumer_keys_are_remapped_to_zmk_usages_and_back() {
        let translator = translator(6);
        let binding = translator.decode(0x00A8);
        assert_eq!(binding.behavior_id, behavior::KP);
        assert_eq!(binding.param1, 0x000C_00E2);
        assert_eq!(translator.encode(binding), Some(0x00A8));
    }

    #[test]
    fn unknown_keycodes_are_preserved_raw() {
        let translator = translator(6);
        let binding = translator.decode(0x7777);
        assert_eq!(binding.behavior_id, behavior::RAW);
        assert_eq!(binding.param1, 0x7777);
        assert_eq!(translator.encode(binding), Some(0x7777));
    }

    #[test]
    fn labels_are_qmk_style() {
        let translator = translator(6);
        assert_eq!(translator.label(translator.decode(0x0004)), "A");
        assert_eq!(translator.label(translator.decode(0x0000)), "KC_NO");
        assert_eq!(translator.label(translator.decode(0x0211)), "LSFT(N)");
    }

    #[test]
    fn catalog_ids_are_unique_and_nonzero() {
        let catalog = catalog();
        assert_eq!(catalog.len(), 14);
        assert!(catalog.iter().all(|behavior| behavior.id != 0));
    }
}
