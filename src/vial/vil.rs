//! Minimal `.vil` backup support.
//!
//! Vial GUI stores a layout snapshot as JSON: a `layout` array indexed by
//! `[layer][row][column]`, an `encoder_layout`, the layout-options bitmask and
//! the protocol versions. This module writes the same shape (using `0xNNNN`
//! keycodes, which Vial's `AnyKeycode` parser accepts) so a keyboard can be
//! backed up before an editing session and restored afterwards.
//!
//! Restoring is intentionally limited to the keymap. Macros, tap dance,
//! combos, key overrides and QMK settings are separate protocol entries and are
//! not exported; a `.vil` written here is a keymap backup, not a full clone of
//! Vial GUI's state.

use serde_json::{Value, json};

use crate::vial::definition::Matrix;
use crate::vial::protocol::{Result, VialError};
use crate::vial::session::VialKeyboard;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VilDocument {
    pub version: u64,
    pub uid: u64,
    pub layout: Vec<Vec<Vec<u16>>>,
    pub layout_options: i64,
    pub via_protocol: u16,
    pub vial_protocol: u32,
}

impl VilDocument {
    /// Build a backup from the session's current (staged) keymap.
    pub fn from_keyboard<T: crate::vial::protocol::RawHid>(keyboard: &VialKeyboard<T>) -> Self {
        let matrix = keyboard.matrix();
        let mut layout = Vec::with_capacity(keyboard.layer_count());
        for layer in keyboard.raw_keycodes() {
            let mut rows = vec![vec![0u16; usize::from(matrix.cols)]; usize::from(matrix.rows)];
            for (index, key) in keyboard.definition().keys.iter().enumerate() {
                rows[usize::from(key.row)][usize::from(key.col)] = layer[index];
            }
            layout.push(rows);
        }
        Self {
            version: 1,
            uid: u64::from_le_bytes(keyboard.uid()),
            layout,
            layout_options: -1,
            via_protocol: keyboard.info().via,
            vial_protocol: keyboard.info().vial.protocol,
        }
    }

    pub fn to_json(&self) -> Value {
        let layout: Vec<Vec<Vec<String>>> = self
            .layout
            .iter()
            .map(|rows| {
                rows.iter()
                    .map(|row| row.iter().map(|code| format!("0x{code:04X}")).collect())
                    .collect()
            })
            .collect();
        json!({
            "version": self.version,
            "uid": self.uid,
            "layout": layout,
            "encoder_layout": Vec::<Vec<[i32; 2]>>::new(),
            "layout_options": self.layout_options,
            "via_protocol": self.via_protocol,
            "vial_protocol": self.vial_protocol,
        })
    }

    pub fn to_json_string(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.to_json())
            .map_err(|error| VialError::Definition(format!("cannot serialise .vil: {error}")))
    }

    /// Parse the keymap portion of a `.vil` file. String keycodes must be
    /// numeric (`0xNNNN` or decimal); named QMK keycodes from a Vial GUI export
    /// are rejected with a clear error instead of being guessed.
    pub fn from_json(value: &Value, matrix: Matrix) -> Result<Self> {
        let layout = value
            .get("layout")
            .and_then(Value::as_array)
            .ok_or_else(|| VialError::Definition("missing `layout` array".into()))?;
        let mut layers = Vec::with_capacity(layout.len());
        for (layer_index, rows) in layout.iter().enumerate() {
            let rows = rows.as_array().ok_or_else(|| {
                VialError::Definition(format!("layout layer {layer_index} is not an array"))
            })?;
            if rows.len() != usize::from(matrix.rows) {
                return Err(VialError::Definition(format!(
                    "layout layer {layer_index} has {} rows, expected {}",
                    rows.len(),
                    matrix.rows
                )));
            }
            let mut parsed_rows = Vec::with_capacity(rows.len());
            for (row_index, row) in rows.iter().enumerate() {
                let row = row.as_array().ok_or_else(|| {
                    VialError::Definition(format!(
                        "layout layer {layer_index} row {row_index} is not an array"
                    ))
                })?;
                if row.len() != usize::from(matrix.cols) {
                    return Err(VialError::Definition(format!(
                        "layout layer {layer_index} row {row_index} has {} columns, expected {}",
                        row.len(),
                        matrix.cols
                    )));
                }
                let mut parsed = Vec::with_capacity(row.len());
                for cell in row {
                    parsed.push(parse_keycode(cell)?);
                }
                parsed_rows.push(parsed);
            }
            layers.push(parsed_rows);
        }
        Ok(Self {
            version: value.get("version").and_then(Value::as_u64).unwrap_or(1),
            uid: value.get("uid").and_then(Value::as_u64).unwrap_or(0),
            layout: layers,
            layout_options: value
                .get("layout_options")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
            via_protocol: value
                .get("via_protocol")
                .and_then(Value::as_u64)
                .unwrap_or(9) as u16,
            vial_protocol: value
                .get("vial_protocol")
                .and_then(Value::as_u64)
                .unwrap_or(6) as u32,
        })
    }

    /// Stage the backup's keycodes. The keyboard's UID must match unless the
    /// document has no UID; a mismatch usually means the wrong file.
    pub fn apply<T: crate::vial::protocol::RawHid>(
        &self,
        keyboard: &mut VialKeyboard<T>,
    ) -> Result<()> {
        if self.uid != 0 && self.uid != u64::from_le_bytes(keyboard.uid()) {
            return Err(VialError::Definition(format!(
                "backup UID {:016X} does not match keyboard {:016X}",
                self.uid,
                u64::from_le_bytes(keyboard.uid())
            )));
        }
        if self.layout.len() != keyboard.layer_count() {
            return Err(VialError::Definition(format!(
                "backup has {} layers, keyboard has {}",
                self.layout.len(),
                keyboard.layer_count()
            )));
        }
        for (layer, rows) in self.layout.iter().enumerate() {
            for (row, cols) in rows.iter().enumerate() {
                for (col, keycode) in cols.iter().enumerate() {
                    let Some(position) = keyboard.definition().key_index(row as u8, col as u8)
                    else {
                        continue;
                    };
                    keyboard.stage_keycode(layer as u32, position, *keycode)?;
                }
            }
        }
        Ok(())
    }
}

fn parse_keycode(value: &Value) -> Result<u16> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| VialError::Definition(format!("keycode {number} is out of range"))),
        Value::String(text) => {
            let text = text.trim();
            let parsed =
                if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    u16::from_str_radix(hex, 16).ok()
                } else {
                    text.parse::<u16>().ok()
                };
            parsed.ok_or_else(|| {
                VialError::Definition(format!(
                    "keycode `{text}` is not numeric; only `.vil` files written by this editor are supported"
                ))
            })
        }
        other => Err(VialError::Definition(format!(
            "unexpected keycode value {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> VilDocument {
        VilDocument {
            version: 1,
            uid: 0,
            layout: vec![vec![vec![0x0004, 0x0005], vec![0x0006, 0x0007]]],
            layout_options: -1,
            via_protocol: 9,
            vial_protocol: 6,
        }
    }

    #[test]
    fn json_round_trips_through_hex_strings() {
        let document = document();
        let json = document.to_json();
        let parsed = VilDocument::from_json(&json, Matrix { rows: 2, cols: 2 }).unwrap();
        assert_eq!(parsed, document);
    }

    #[test]
    fn rejects_named_keycodes_and_wrong_dimensions() {
        let named = json!({"layout": [[["KC_A", "0x0005"]]]});
        let error = VilDocument::from_json(&named, Matrix { rows: 1, cols: 2 }).unwrap_err();
        assert!(error.to_string().contains("not numeric"));
        let wrong = json!({"layout": [[["0x0004"]]]});
        let error = VilDocument::from_json(&wrong, Matrix { rows: 1, cols: 2 }).unwrap_err();
        assert!(error.to_string().contains("columns"));
    }
}
