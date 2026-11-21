//! In-process Vial keyboard used by tests, the probe example and `--simulate`.
//!
//! The simulator mirrors `quantum/via.c` and `quantum/vial.c`: fixed 32-byte
//! reports, a dynamic keymap in EEPROM order (layer, row, column), the embedded
//! XZ-compressed keyboard definition, the keyboard UID and Vial's unlock
//! gesture. It is deliberately strict about offsets so protocol regressions
//! show up without hardware.

use std::collections::HashSet;
use std::io::Cursor;

use crate::vial::definition::Matrix;
use crate::vial::protocol::{
    BUFFER_CHUNK, HID_USAGE, HID_USAGE_PAGE, REPORT_SIZE, RawHid, Result, SERIAL_MAGIC,
    VIA_PROTOCOL_VERSION, VialError, command, vial_command,
};

/// Small keyboard used by the bundled probe: 4 rows by 6 columns.
pub const DEMO_ROWS: u8 = 4;
pub const DEMO_COLS: u8 = 6;
pub const DEMO_LAYERS: u8 = 4;

pub struct SimulatedVial {
    matrix: Matrix,
    layers: u8,
    /// Layer-major, row-major keycodes, exactly as stored in EEPROM.
    keymap: Vec<u16>,
    initial_keymap: Vec<u16>,
    definition: Vec<u8>,
    uid: [u8; 8],
    via_protocol: u16,
    vial_protocol: u32,
    unlocked: bool,
    unlock_in_progress: bool,
    unlock_counter: u8,
    unlock_keys: Vec<(u8, u8)>,
    held: HashSet<(u8, u8)>,
    fail_next: Option<String>,
    /// Audit log of accepted `set_keycode` writes.
    pub writes: Vec<(u8, u8, u8, u16)>,
}

impl SimulatedVial {
    /// A 4x6, 4-layer keyboard with letter keys on the base layer and
    /// transparent keys above it.
    pub fn demo() -> Self {
        Self::new(DEMO_ROWS, DEMO_COLS, DEMO_LAYERS)
    }

    pub fn new(rows: u8, cols: u8, layers: u8) -> Self {
        let matrix = Matrix { rows, cols };
        let definition_json = simple_definition_json(rows, cols, layers);
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut Cursor::new(definition_json), &mut compressed)
            .expect("compress simulated definition");
        let mut keymap = vec![0u16; usize::from(layers) * matrix.cell_count()];
        for (index, keycode) in keymap.iter_mut().take(matrix.cell_count()).enumerate() {
            *keycode = 0x04 + (index as u16 % 26);
        }
        for keycode in keymap.iter_mut().skip(matrix.cell_count()) {
            *keycode = 0x0001;
        }
        let initial_keymap = keymap.clone();
        Self {
            matrix,
            layers,
            keymap,
            initial_keymap,
            definition: compressed,
            uid: *b"VIALDEMO",
            via_protocol: VIA_PROTOCOL_VERSION,
            vial_protocol: 6,
            // Vial firmware boots locked unless built with VIAL_INSECURE.
            unlocked: false,
            unlock_in_progress: false,
            unlock_counter: 0,
            unlock_keys: vec![(0, 0), (0, 1)],
            held: HashSet::new(),
            fail_next: None,
            writes: Vec::new(),
        }
    }

    pub fn with_unlocked(mut self, unlocked: bool) -> Self {
        self.unlocked = unlocked;
        self
    }

    pub fn with_vial_protocol(mut self, protocol: u32) -> Self {
        self.vial_protocol = protocol;
        self
    }

    pub fn with_unlock_keys(mut self, keys: &[(u8, u8)]) -> Self {
        self.unlock_keys = keys.to_vec();
        self
    }

    pub fn matrix(&self) -> Matrix {
        self.matrix
    }

    pub fn layers(&self) -> u8 {
        self.layers
    }

    pub fn uid(&self) -> [u8; 8] {
        self.uid
    }

    /// HID serial string a real device would advertise.
    pub fn serial(&self) -> String {
        format!("{SERIAL_MAGIC}{}", self.uid_hex())
    }

    pub fn uid_hex(&self) -> String {
        self.uid.iter().map(|byte| format!("{byte:02X}")).collect()
    }

    pub fn hid_descriptor(&self) -> (u16, u16) {
        (HID_USAGE_PAGE, HID_USAGE)
    }

    pub fn keycode(&self, layer: u8, row: u8, col: u8) -> u16 {
        self.keymap[self.index(layer, row, col)]
    }

    pub fn set_keycode(&mut self, layer: u8, row: u8, col: u8, keycode: u16) {
        let index = self.index(layer, row, col);
        self.keymap[index] = keycode;
    }

    pub fn definition_json(&self) -> Vec<u8> {
        let mut json = Vec::new();
        lzma_rs::xz_decompress(&mut Cursor::new(self.definition.clone()), &mut json)
            .expect("decompress simulated definition");
        json
    }

    /// Simulate the user holding the unlock keys (and releasing all others).
    pub fn hold_keys(&mut self, keys: &[(u8, u8)]) {
        self.held.clear();
        self.held.extend(keys.iter().copied());
    }

    pub fn release_keys(&mut self) {
        self.held.clear();
    }

    /// Make the next exchange fail, to exercise transport error paths.
    pub fn fail_next(&mut self, message: impl Into<String>) {
        self.fail_next = Some(message.into());
    }

    fn index(&self, layer: u8, row: u8, col: u8) -> usize {
        usize::from(layer) * self.matrix.cell_count()
            + usize::from(row) * usize::from(self.matrix.cols)
            + usize::from(col)
    }

    fn valid(&self, layer: u8, row: u8, col: u8) -> bool {
        layer < self.layers && row < self.matrix.rows && col < self.matrix.cols
    }

    fn unlock_status_report(&self) -> [u8; REPORT_SIZE] {
        let mut response = [0xFFu8; REPORT_SIZE];
        response[0] = u8::from(self.unlocked);
        response[1] = u8::from(self.unlock_in_progress);
        for (index, &(row, col)) in self.unlock_keys.iter().take(15).enumerate() {
            response[2 + index * 2] = row;
            response[3 + index * 2] = col;
        }
        response
    }

    fn poll_unlock(&mut self) -> [u8; REPORT_SIZE] {
        if self.unlock_in_progress {
            let all_held = self
                .unlock_keys
                .iter()
                .all(|position| self.held.contains(position));
            if all_held {
                self.unlock_counter = self.unlock_counter.saturating_sub(1);
                if self.unlock_counter == 0 {
                    self.unlocked = true;
                    self.unlock_in_progress = false;
                }
            } else {
                self.unlock_counter = 3;
            }
        }
        let mut response = [0u8; REPORT_SIZE];
        response[0] = u8::from(self.unlocked);
        response[1] = u8::from(self.unlock_in_progress);
        response[2] = self.unlock_counter;
        response
    }
}

impl RawHid for SimulatedVial {
    fn exchange(&mut self, request: [u8; REPORT_SIZE]) -> Result<[u8; REPORT_SIZE]> {
        if let Some(message) = self.fail_next.take() {
            return Err(VialError::Transport(message));
        }
        let mut response = request;
        match request[0] {
            command::GET_PROTOCOL_VERSION => {
                let [hi, lo] = self.via_protocol.to_be_bytes();
                response[1] = hi;
                response[2] = lo;
            }
            command::DYNAMIC_KEYMAP_GET_KEYCODE => {
                let (layer, row, col) = (request[1], request[2], request[3]);
                let keycode = if self.valid(layer, row, col) {
                    self.keycode(layer, row, col)
                } else {
                    0
                };
                let [hi, lo] = keycode.to_be_bytes();
                response[4] = hi;
                response[5] = lo;
            }
            command::DYNAMIC_KEYMAP_SET_KEYCODE => {
                let (layer, row, col) = (request[1], request[2], request[3]);
                let keycode = u16::from_be_bytes([request[4], request[5]]);
                if self.valid(layer, row, col) {
                    let index = self.index(layer, row, col);
                    self.keymap[index] = keycode;
                    self.writes.push((layer, row, col, keycode));
                }
            }
            command::DYNAMIC_KEYMAP_RESET => {
                self.keymap.clone_from(&self.initial_keymap);
            }
            command::GET_LAYER_COUNT => {
                response[1] = self.layers;
            }
            command::KEYMAP_GET_BUFFER => {
                let offset = u16::from_be_bytes([request[1], request[2]]) as usize;
                let size = usize::from(request[3]).min(BUFFER_CHUNK);
                for index in 0..size {
                    let byte = offset + index;
                    response[4 + index] = if byte < self.keymap.len() * 2 {
                        self.keymap[byte / 2].to_be_bytes()[byte % 2]
                    } else {
                        0
                    };
                }
            }
            command::MACRO_GET_COUNT => {
                response[1] = 16;
            }
            command::MACRO_GET_BUFFER_SIZE => {
                let [hi, lo] = 0u16.to_be_bytes();
                response[1] = hi;
                response[2] = lo;
            }
            command::VIAL => match request[1] {
                vial_command::GET_KEYBOARD_ID => {
                    response[..4].copy_from_slice(&self.vial_protocol.to_le_bytes());
                    response[4..12].copy_from_slice(&self.uid);
                    response[12] = 0;
                }
                vial_command::GET_SIZE => {
                    response[..4].copy_from_slice(&(self.definition.len() as u32).to_le_bytes());
                }
                vial_command::GET_DEFINITION => {
                    let page = usize::from(request[2]) | usize::from(request[3]) << 8;
                    let start = page * REPORT_SIZE;
                    response = [0u8; REPORT_SIZE];
                    if start < self.definition.len() {
                        let end = (start + REPORT_SIZE).min(self.definition.len());
                        response[..end - start].copy_from_slice(&self.definition[start..end]);
                    }
                }
                vial_command::GET_UNLOCK_STATUS => {
                    response = self.unlock_status_report();
                }
                vial_command::UNLOCK_START => {
                    self.unlock_in_progress = true;
                    self.unlock_counter = 3;
                }
                vial_command::UNLOCK_POLL => {
                    response = self.poll_unlock();
                }
                vial_command::LOCK => {
                    self.unlocked = false;
                }
                vial_command::GET_ENCODER => {
                    response[0] = 0;
                    response[1] = 0;
                    response[2] = 0;
                    response[3] = 0;
                }
                vial_command::SET_ENCODER => {}
                _ => {
                    response[0] = 0xFF;
                }
            },
            _ => {
                response[0] = 0xFF;
            }
        }
        Ok(response)
    }
}

/// KLE JSON with a plain grid; one key per matrix cell.
fn simple_definition_json(rows: u8, cols: u8, layers: u8) -> Vec<u8> {
    let mut rows_json = Vec::new();
    for row in 0..rows {
        let keys: Vec<String> = (0..cols).map(|col| format!("\"{row},{col}\"")).collect();
        rows_json.push(format!("[{}]", keys.join(", ")));
    }
    format!(
        r#"{{"name":"Simulated Vial {rows}x{cols}","matrix":{{"rows":{rows},"cols":{cols}}},"layouts":{{"keymap":[{}]}},"vial":{{"layers":{layers}}}}}"#,
        rows_json.join(",")
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vial::protocol::report;

    #[test]
    fn simulator_reports_the_vial_identity_and_definition() {
        let mut simulator = SimulatedVial::demo();
        let version = simulator
            .exchange(report(&[command::GET_PROTOCOL_VERSION]))
            .unwrap();
        assert_eq!(u16::from_be_bytes([version[1], version[2]]), 9);
        let id = simulator
            .exchange(report(&[command::VIAL, vial_command::GET_KEYBOARD_ID]))
            .unwrap();
        assert_eq!(u32::from_le_bytes(id[..4].try_into().unwrap()), 6);
        assert_eq!(&id[4..12], b"VIALDEMO");
    }

    #[test]
    fn simulator_keymap_is_layer_major_and_big_endian() {
        let mut simulator = SimulatedVial::demo();
        let response = simulator
            .exchange(report(&[command::KEYMAP_GET_BUFFER, 0, 0, 4]))
            .unwrap();
        assert_eq!(&response[4..8], &[0x00, 0x04, 0x00, 0x05]);
    }

    #[test]
    fn unlock_requires_the_requested_keys_and_three_polls() {
        let mut simulator = SimulatedVial::demo();
        simulator.hold_keys(&[(0, 0), (0, 1)]);
        simulator
            .exchange(report(&[command::VIAL, vial_command::UNLOCK_START]))
            .unwrap();
        for expected in [2u8, 1, 0] {
            let response = simulator
                .exchange(report(&[command::VIAL, vial_command::UNLOCK_POLL]))
                .unwrap();
            assert_eq!(response[2], expected);
        }
        assert!(simulator.unlocked);
    }

    #[test]
    fn auxiliary_commands_and_getters_are_exercised() {
        let mut simulator = SimulatedVial::demo()
            .with_vial_protocol(5)
            .with_unlocked(true)
            .with_unlock_keys(&[(1, 2)]);
        assert_eq!(
            simulator.matrix(),
            Matrix {
                rows: DEMO_ROWS,
                cols: DEMO_COLS
            }
        );
        assert_eq!(simulator.layers(), DEMO_LAYERS);
        assert_eq!(simulator.uid(), *b"VIALDEMO");
        assert_eq!(simulator.uid_hex(), "5649414C44454D4F");
        assert_eq!(simulator.serial(), "vial:f64c2b3c5649414C44454D4F");
        assert_eq!(simulator.hid_descriptor(), (HID_USAGE_PAGE, HID_USAGE));
        assert!(!simulator.definition_json().is_empty());

        simulator.set_keycode(1, 2, 3, 0x1234);
        assert_eq!(simulator.keycode(1, 2, 3), 0x1234);

        let macro_count = simulator
            .exchange(report(&[command::MACRO_GET_COUNT]))
            .unwrap();
        assert_eq!(macro_count[1], 16);
        let macro_size = simulator
            .exchange(report(&[command::MACRO_GET_BUFFER_SIZE]))
            .unwrap();
        assert_eq!(&macro_size[1..3], &[0, 0]);

        let encoder = simulator
            .exchange(report(&[command::VIAL, vial_command::GET_ENCODER]))
            .unwrap();
        assert_eq!(&encoder[..4], &[0, 0, 0, 0]);
        simulator
            .exchange(report(&[command::VIAL, vial_command::SET_ENCODER]))
            .unwrap();
        let unknown_vial = simulator.exchange(report(&[command::VIAL, 0x3F])).unwrap();
        assert_eq!(unknown_vial[0], 0xFF);
        let unknown = simulator.exchange(report(&[0x7F])).unwrap();
        assert_eq!(unknown[0], 0xFF);

        simulator
            .exchange(report(&[command::VIAL, vial_command::LOCK]))
            .unwrap();
        assert!(!simulator.unlocked);
    }

    #[test]
    fn invalid_cells_and_definition_pages_are_bounded() {
        let mut simulator = SimulatedVial::demo();
        let invalid = simulator
            .exchange(report(&[command::DYNAMIC_KEYMAP_GET_KEYCODE, 99, 99, 99]))
            .unwrap();
        assert_eq!(&invalid[4..6], &[0, 0]);
        simulator
            .exchange(report(&[
                command::DYNAMIC_KEYMAP_SET_KEYCODE,
                99,
                0,
                0,
                0,
                4,
            ]))
            .unwrap();
        assert!(simulator.writes.is_empty());

        let beyond = simulator
            .exchange(report(&[command::KEYMAP_GET_BUFFER, 0xFF, 0xFF, 4]))
            .unwrap();
        assert_eq!(&beyond[4..8], &[0, 0, 0, 0]);
        let page = simulator
            .exchange(report(&[
                command::VIAL,
                vial_command::GET_DEFINITION,
                0xFF,
                0xFF,
            ]))
            .unwrap();
        assert_eq!(page, [0u8; REPORT_SIZE]);

        simulator.fail_next("cable unplugged");
        let error = simulator
            .exchange(report(&[command::GET_LAYER_COUNT]))
            .unwrap_err();
        assert!(matches!(error, VialError::Transport(_)));
    }

    #[test]
    fn releasing_unlock_keys_resets_the_hold_counter() {
        let mut simulator = SimulatedVial::demo().with_unlock_keys(&[(0, 0)]);
        simulator
            .exchange(report(&[command::VIAL, vial_command::UNLOCK_START]))
            .unwrap();
        simulator.hold_keys(&[(0, 0)]);
        let first = simulator
            .exchange(report(&[command::VIAL, vial_command::UNLOCK_POLL]))
            .unwrap();
        assert_eq!(first[2], 2);
        simulator.release_keys();
        let reset = simulator
            .exchange(report(&[command::VIAL, vial_command::UNLOCK_POLL]))
            .unwrap();
        assert_eq!(reset[2], 3);
        assert!(!simulator.unlocked);
    }

    #[test]
    fn writes_are_audited() {
        let mut simulator = SimulatedVial::demo();
        simulator
            .exchange(report(&[0x05, 0, 0, 0, 0x00, 0x04]))
            .unwrap();
        assert_eq!(simulator.keycode(0, 0, 0), 4);
        assert_eq!(simulator.writes, vec![(0, 0, 0, 4)]);
    }
}
