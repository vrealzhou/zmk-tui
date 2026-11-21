//! Parse the keyboard definition embedded in Vial firmware.
//!
//! `vial-qmk` compresses a `vial.json` document into the firmware image and
//! serves it through `vial_get_size` / `vial_get_def`. The document contains the
//! switch matrix size and a Keyboard-Layout-Editor (KLE) layout whose keys are
//! tagged with their matrix position in the first legend (`"row,col"`).
//!
//! This module implements the subset of KLE that Vial definitions use:
//! relative `x`/`y`, `w`/`h`, decals (spacers) and layout-option legends. Keys
//! that share a matrix cell because they belong to different layout options
//! are deduplicated; the first occurrence wins. Encoder entries are counted but
//! not exposed (the Studio model used by this editor has no encoder bindings).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::{KeyGeometry, PhysicalLayout};
use crate::vial::protocol::{Result, VialError};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Matrix {
    pub rows: u8,
    pub cols: u8,
}

impl Matrix {
    pub fn cell_count(self) -> usize {
        usize::from(self.rows) * usize::from(self.cols)
    }
}

/// One physical key and the matrix cell it reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyPosition {
    pub row: u8,
    pub col: u8,
    pub geometry: KeyGeometry,
}

/// Parsed `vial.json` data needed by the editor.
#[derive(Debug, Clone, Default)]
pub struct KeyboardDefinition {
    pub name: String,
    pub matrix: Matrix,
    /// Physical keys in KLE order, deduplicated by matrix cell.
    pub keys: Vec<KeyPosition>,
    /// Number of duplicate matrix cells (layout options) that were dropped.
    pub duplicates: usize,
    /// Number of encoder entries found in the layout.
    pub encoders: usize,
    index_by_cell: BTreeMap<(u8, u8), usize>,
}

impl KeyboardDefinition {
    pub fn parse(json: &[u8]) -> Result<Self> {
        let document: Value = serde_json::from_slice(json).map_err(|error| {
            VialError::Definition(format!("definition is not valid JSON: {error}"))
        })?;
        Self::from_value(&document)
    }

    fn from_value(document: &Value) -> Result<Self> {
        let matrix = document
            .get("matrix")
            .ok_or_else(|| VialError::Definition("missing `matrix` object".into()))?;
        let rows = dimension(matrix, "rows")?;
        let cols = dimension(matrix, "cols")?;
        let matrix = Matrix { rows, cols };

        let keymap = document
            .get("layouts")
            .and_then(|layouts| layouts.get("keymap"))
            .and_then(Value::as_array)
            .ok_or_else(|| VialError::Definition("missing `layouts.keymap` array".into()))?;

        let name = document
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Vial keyboard")
            .to_string();

        let mut definition = Self {
            name,
            matrix,
            ..Default::default()
        };

        // KLE state machine: an object mutates the next key's state; a string
        // is a key. The cursor advances by the key width and y grows by one per
        // row, exactly like kle-serial's deserialize().
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut width = 1.0f32;
        let mut height = 1.0f32;
        let mut decal = false;
        let mut seen = BTreeMap::<(u8, u8), usize>::new();

        for row in keymap {
            let Some(items) = row.as_array() else {
                continue;
            };
            for item in items {
                if let Some(object) = item.as_object() {
                    x += number(object.get("x"));
                    y += number(object.get("y"));
                    if let Some(value) = object.get("w").and_then(Value::as_f64) {
                        width = value as f32;
                    }
                    if let Some(value) = object.get("h").and_then(Value::as_f64) {
                        height = value as f32;
                    }
                    if object.get("d").and_then(Value::as_bool).unwrap_or(false) {
                        decal = true;
                    }
                    continue;
                }
                let Some(text) = item.as_str() else {
                    continue;
                };
                let labels: Vec<&str> = text.split('\n').collect();
                if labels.iter().any(|label| label.trim() == "e") {
                    // Encoder entries are tagged separately by Vial.
                    definition.encoders += 1;
                } else if !decal {
                    if let Some((row, col)) = matrix_label(labels.first().copied().unwrap_or("")) {
                        if row >= matrix.rows || col >= matrix.cols {
                            return Err(VialError::Definition(format!(
                                "key references matrix cell {row},{col} but the matrix is {}x{}",
                                matrix.rows, matrix.cols
                            )));
                        }
                        match seen.entry((row, col)) {
                            std::collections::btree_map::Entry::Occupied(_) => {
                                // Layout options reuse a cell; keep one geometry.
                                definition.duplicates += 1;
                            }
                            std::collections::btree_map::Entry::Vacant(entry) => {
                                let index = definition.keys.len();
                                entry.insert(index);
                                definition.keys.push(KeyPosition {
                                    row,
                                    col,
                                    geometry: KeyGeometry {
                                        x,
                                        y,
                                        width,
                                        height,
                                        ..Default::default()
                                    },
                                });
                            }
                        }
                    }
                }
                x += width;
                width = 1.0;
                height = 1.0;
                decal = false;
            }
            y += 1.0;
            x = 0.0;
        }

        if definition.keys.is_empty() {
            return Err(VialError::Definition(
                "layout contains no key with a matrix position".into(),
            ));
        }
        definition.index_by_cell = seen;
        Ok(definition)
    }

    pub fn key_index(&self, row: u8, col: u8) -> Option<usize> {
        self.index_by_cell.get(&(row, col)).copied()
    }

    /// Geometry in KLE order, ready for the Studio physical-layout model.
    pub fn physical_layout(&self) -> PhysicalLayout {
        PhysicalLayout {
            name: self.name.clone(),
            keys: self.keys.iter().map(|key| key.geometry).collect(),
        }
    }

    /// Bounding box, used by tests and diagnostics.
    pub fn bounds(&self) -> (f32, f32) {
        let mut width = 0.0f32;
        let mut height = 0.0f32;
        for key in &self.keys {
            width = width.max(key.geometry.x + key.geometry.width);
            height = height.max(key.geometry.y + key.geometry.height);
        }
        (width, height)
    }
}

fn dimension(matrix: &Value, field: &str) -> Result<u8> {
    let value = matrix
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| VialError::Definition(format!("missing matrix.{field}")))?;
    u8::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| VialError::Definition(format!("matrix.{field} out of range")))
}

fn number(value: Option<&Value>) -> f32 {
    value.and_then(Value::as_f64).unwrap_or(0.0) as f32
}

fn matrix_label(label: &str) -> Option<(u8, u8)> {
    let (row, col) = label.split_once(',')?;
    Some((row.trim().parse().ok()?, col.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_grid_in_kle_order() {
        let json = br##"{
            "matrix": {"rows": 2, "cols": 2},
            "layouts": {"keymap": [["0,0", "0,1"], ["1,0", "1,1"]]}
        }"##;
        let definition = KeyboardDefinition::parse(json).unwrap();
        assert_eq!(definition.matrix, Matrix { rows: 2, cols: 2 });
        assert_eq!(definition.keys.len(), 4);
        assert_eq!(definition.key_index(1, 0), Some(2));
        assert_eq!(definition.keys[3].geometry.x, 1.0);
        assert_eq!(definition.keys[3].geometry.y, 1.0);
        assert_eq!(definition.physical_layout().keys.len(), 4);
    }

    #[test]
    fn applies_offsets_widths_and_decals() {
        let json = br##"{
            "name": "Offset",
            "matrix": {"rows": 1, "cols": 3},
            "layouts": {"keymap": [[
                {"x": 2.5, "w": 1.5, "c": "#777777"},
                "0,0",
                {"d": true},
                "decal",
                "0,1",
                "0,2"
            ]]}
        }"##;
        let definition = KeyboardDefinition::parse(json).unwrap();
        assert_eq!(definition.name, "Offset");
        assert_eq!(definition.keys.len(), 3);
        assert_eq!(definition.keys[0].geometry.x, 2.5);
        assert_eq!(definition.keys[0].geometry.width, 1.5);
        assert_eq!(definition.keys[1].geometry.x, 5.0);
        assert_eq!(definition.keys[2].geometry.x, 6.0);
        assert_eq!(definition.bounds(), (7.0, 1.0));
    }

    #[test]
    fn duplicate_matrix_cells_are_deduplicated() {
        let json = br##"{
            "matrix": {"rows": 1, "cols": 2},
            "layouts": {"keymap": [[
                "0,0", "0,1\n\n\n0,1", "0,1\n\n\n0,0"
            ]]}
        }"##;
        let definition = KeyboardDefinition::parse(json).unwrap();
        assert_eq!(definition.keys.len(), 2);
        assert_eq!(definition.duplicates, 1);
    }

    #[test]
    fn rejects_missing_matrix_and_out_of_range_cells() {
        let missing = br##"{"layouts": {"keymap": [["0,0"]]}}"##;
        assert!(KeyboardDefinition::parse(missing).is_err());
        let out_of_range = br##"{
            "matrix": {"rows": 1, "cols": 1},
            "layouts": {"keymap": [["0,4"]]}
        }"##;
        let error = KeyboardDefinition::parse(out_of_range).unwrap_err();
        assert!(error.to_string().contains("0,4"));
    }

    #[test]
    fn counts_encoders_without_adding_them_as_keys() {
        let json = br##"{
            "matrix": {"rows": 1, "cols": 2},
            "layouts": {"keymap": [[
                "0,0",
                "0,1\n\n\ne"
            ]]}
        }"##;
        let definition = KeyboardDefinition::parse(json).unwrap();
        assert_eq!(definition.keys.len(), 1);
        assert_eq!(definition.encoders, 1);
    }
}
