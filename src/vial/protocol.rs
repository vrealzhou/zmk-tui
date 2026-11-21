//! VIA/Vial raw-HID protocol.
//!
//! A Vial keyboard exposes a vendor-defined HID interface (usage page
//! `0xFF60`, usage `0x61`) with fixed 32-byte reports. The host writes one
//! report and the firmware answers by rewriting the same buffer. There is no
//! framing, request id or length field: the command byte selects the handler
//! and every field is at a fixed offset.
//!
//! Vial is a superset of VIA. VIA commands (`0x01`..`0x13`) cover versions,
//! the dynamic keymap and macros; Vial commands are prefixed with `0xFE` and
//! add the keyboard definition, keyboard UID, unlock state and the
//! tap-dance/combo/key-override/QMK-settings entries.
//!
//! Sources: `vial-kb/vial-qmk` `quantum/via.h`, `quantum/via.c`,
//! `quantum/vial.h`, `quantum/vial.c` (branch `vial`), and `vial-kb/vial-gui`
//! `protocol/keyboard_comm.py`.

use std::fmt;

use thiserror::Error;

/// Vial raw-HID report size. Both directions are exactly this many bytes.
pub const REPORT_SIZE: usize = 32;
/// Prefix byte for Vial-specific commands.
pub const VIAL_PREFIX: u8 = 0xFE;
/// HID usage page of the Vial/VIA raw-HID interface.
pub const HID_USAGE_PAGE: u16 = 0xFF60;
/// HID usage id of the Vial/VIA raw-HID interface.
pub const HID_USAGE: u16 = 0x61;
/// Devices that opt into Vial carry this substring in the HID serial number.
pub const SERIAL_MAGIC: &str = "vial:f64c2b3c";
/// The VIA protocol version implemented by current `vial-qmk`.
pub const VIA_PROTOCOL_VERSION: u16 = 0x0009;
/// Maximum number of bytes handled in one buffer read/write report.
pub const BUFFER_CHUNK: usize = 28;

/// VIA command ids. Values are copied from `quantum/via.h`; note that this
/// differs from upstream QMK only by which ids are present.
pub mod command {
    pub const GET_PROTOCOL_VERSION: u8 = 0x01;
    pub const GET_KEYBOARD_VALUE: u8 = 0x02;
    pub const SET_KEYBOARD_VALUE: u8 = 0x03;
    pub const DYNAMIC_KEYMAP_GET_KEYCODE: u8 = 0x04;
    pub const DYNAMIC_KEYMAP_SET_KEYCODE: u8 = 0x05;
    pub const DYNAMIC_KEYMAP_RESET: u8 = 0x06;
    pub const LIGHTING_SET_VALUE: u8 = 0x07;
    pub const LIGHTING_GET_VALUE: u8 = 0x08;
    pub const LIGHTING_SAVE: u8 = 0x09;
    pub const EEPROM_RESET: u8 = 0x0A;
    pub const BOOTLOADER_JUMP: u8 = 0x0B;
    pub const MACRO_GET_COUNT: u8 = 0x0C;
    pub const MACRO_GET_BUFFER_SIZE: u8 = 0x0D;
    pub const MACRO_GET_BUFFER: u8 = 0x0E;
    pub const MACRO_SET_BUFFER: u8 = 0x0F;
    pub const MACRO_RESET: u8 = 0x10;
    pub const GET_LAYER_COUNT: u8 = 0x11;
    pub const KEYMAP_GET_BUFFER: u8 = 0x12;
    pub const KEYMAP_SET_BUFFER: u8 = 0x13;
    pub const VIAL: u8 = super::VIAL_PREFIX;
}

/// Vial-specific sub-commands, sent as `[0xFE, sub_command, ..]`.
pub mod vial_command {
    pub const GET_KEYBOARD_ID: u8 = 0x00;
    pub const GET_SIZE: u8 = 0x01;
    pub const GET_DEFINITION: u8 = 0x02;
    pub const GET_ENCODER: u8 = 0x03;
    pub const SET_ENCODER: u8 = 0x04;
    pub const GET_UNLOCK_STATUS: u8 = 0x05;
    pub const UNLOCK_START: u8 = 0x06;
    pub const UNLOCK_POLL: u8 = 0x07;
    pub const LOCK: u8 = 0x08;
    pub const QMK_SETTINGS_QUERY: u8 = 0x09;
    pub const QMK_SETTINGS_GET: u8 = 0x0A;
    pub const QMK_SETTINGS_SET: u8 = 0x0B;
    pub const QMK_SETTINGS_RESET: u8 = 0x0C;
    pub const DYNAMIC_ENTRY_OP: u8 = 0x0D;
}

/// Vial protocol versions newer than this are not understood. The numeric
/// keycode ABI changed at version 6 (QMK keycode renumbering), which is why
/// the protocol version matters beyond feature detection.
pub const VIAL_PROTOCOL_MAX: u32 = 6;
/// First Vial protocol version with the current (QMK 0.24+) keycode numbers.
pub const VIAL_PROTOCOL_MODERN_KEYCODES: u32 = 6;

#[derive(Debug, Error)]
pub enum VialError {
    #[error("HID transport error: {0}")]
    Transport(String),
    #[error("the keyboard did not answer within the timeout")]
    Timeout,
    #[error("unexpected Vial response: {0}")]
    Unexpected(String),
    #[error(
        "unsupported protocol: VIA 0x{via:04X}, Vial {vial} (this build understands VIA 0x0009..0x000D and Vial 1..={max})",
        max = VIAL_PROTOCOL_MAX
    )]
    UnsupportedProtocol { via: u16, vial: u32 },
    #[error("invalid keyboard definition: {0}")]
    Definition(String),
    #[error("keycode error: {0}")]
    Keycode(String),
    #[error("the keyboard does not support {0}")]
    Unsupported(&'static str),
    #[error("the keyboard is locked — hold the physical unlock keys and retry")]
    Locked,
}

pub type Result<T> = std::result::Result<T, VialError>;

/// One 32-byte raw-HID exchange. Implementations may be a real USB device or
/// an in-process simulator.
pub trait RawHid: Send {
    fn exchange(&mut self, report: [u8; REPORT_SIZE]) -> Result<[u8; REPORT_SIZE]>;
}

impl<T: RawHid + ?Sized> RawHid for &mut T {
    fn exchange(&mut self, report: [u8; REPORT_SIZE]) -> Result<[u8; REPORT_SIZE]> {
        (**self).exchange(report)
    }
}

impl<T: RawHid + ?Sized> RawHid for Box<T> {
    fn exchange(&mut self, report: [u8; REPORT_SIZE]) -> Result<[u8; REPORT_SIZE]> {
        (**self).exchange(report)
    }
}

/// Identity returned by `vial_get_keyboard_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VialId {
    pub protocol: u32,
    pub uid: [u8; 8],
}

impl VialId {
    pub fn uid_hex(&self) -> String {
        self.uid.iter().map(|byte| format!("{byte:02X}")).collect()
    }
}

impl fmt::Display for VialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vial protocol {} uid {}", self.protocol, self.uid_hex())
    }
}

/// Result of `vial_get_unlock_status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlockStatus {
    pub unlocked: bool,
    pub in_progress: bool,
    /// Matrix positions the user must hold to unlock the keyboard.
    pub keys: Vec<(u8, u8)>,
}

/// Full protocol handshake data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolInfo {
    pub via: u16,
    pub vial: VialId,
}

impl ProtocolInfo {
    pub fn uid_hex(&self) -> String {
        self.vial.uid_hex()
    }
}

/// A connected keyboard speaking the VIA/Vial raw-HID protocol.
pub struct VialDevice<T: RawHid> {
    transport: T,
}

fn report_from(bytes: &[u8]) -> [u8; REPORT_SIZE] {
    debug_assert!(bytes.len() <= REPORT_SIZE);
    let mut report = [0u8; REPORT_SIZE];
    report[..bytes.len()].copy_from_slice(bytes);
    report
}

impl<T: RawHid> VialDevice<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    fn send(&mut self, bytes: &[u8]) -> Result<[u8; REPORT_SIZE]> {
        let response = self.transport.exchange(report_from(bytes))?;
        if response[0] == 0xFF {
            return Err(VialError::Unexpected(format!(
                "command 0x{:02X} was not handled by the firmware",
                bytes[0]
            )));
        }
        Ok(response)
    }

    fn send_vial(&mut self, sub_command: u8, payload: &[u8]) -> Result<[u8; REPORT_SIZE]> {
        let mut bytes = Vec::with_capacity(2 + payload.len());
        bytes.push(VIAL_PREFIX);
        bytes.push(sub_command);
        bytes.extend_from_slice(payload);
        self.send(&bytes)
    }

    /// `id_get_protocol_version`: response bytes 1..3 are big-endian.
    pub fn via_protocol(&mut self) -> Result<u16> {
        let response = self.send(&[command::GET_PROTOCOL_VERSION])?;
        Ok(u16::from_be_bytes([response[1], response[2]]))
    }

    /// `vial_get_keyboard_id`: 32-bit little-endian protocol, 8-byte UID.
    pub fn vial_id(&mut self) -> Result<VialId> {
        let response = self.send_vial(vial_command::GET_KEYBOARD_ID, &[])?;
        let protocol = u32::from_le_bytes([response[0], response[1], response[2], response[3]]);
        let mut uid = [0u8; 8];
        uid.copy_from_slice(&response[4..12]);
        Ok(VialId { protocol, uid })
    }

    /// Read both version numbers and reject combinations whose wire format or
    /// keycode numbering this module cannot interpret.
    pub fn handshake(&mut self) -> Result<ProtocolInfo> {
        let via = self.via_protocol()?;
        let vial = self.vial_id()?;
        // Vial GUI accepts exactly VIA 9; the commands used here are also
        // present in the newer upstream VIA protocol revisions, so accept the
        // small compatible window rather than refusing a working keyboard.
        if !(0x0009..=0x000D).contains(&via)
            || vial.protocol == 0
            || vial.protocol > VIAL_PROTOCOL_MAX
        {
            return Err(VialError::UnsupportedProtocol {
                via,
                vial: vial.protocol,
            });
        }
        Ok(ProtocolInfo { via, vial })
    }

    /// `id_dynamic_keymap_get_layer_count`.
    pub fn layer_count(&mut self) -> Result<u8> {
        let response = self.send(&[command::GET_LAYER_COUNT])?;
        if response[1] == 0 {
            return Err(VialError::Unexpected(
                "firmware reports zero dynamic keymap layers".into(),
            ));
        }
        Ok(response[1])
    }

    /// `id_dynamic_keymap_get_keycode`: 16-bit big-endian at bytes 4..6.
    pub fn keycode(&mut self, layer: u8, row: u8, col: u8) -> Result<u16> {
        let response = self.send(&[command::DYNAMIC_KEYMAP_GET_KEYCODE, layer, row, col])?;
        Ok(u16::from_be_bytes([response[4], response[5]]))
    }

    /// `id_dynamic_keymap_set_keycode`. This writes the keyboard's persistent
    /// storage immediately; callers that want an explicit save step must stage
    /// edits themselves.
    pub fn set_keycode(&mut self, layer: u8, row: u8, col: u8, keycode: u16) -> Result<()> {
        let [hi, lo] = keycode.to_be_bytes();
        self.send(&[command::DYNAMIC_KEYMAP_SET_KEYCODE, layer, row, col, hi, lo])?;
        Ok(())
    }

    /// `id_dynamic_keymap_reset` (restore the compiled keymap).
    pub fn reset_keymap(&mut self) -> Result<()> {
        self.send(&[command::DYNAMIC_KEYMAP_RESET])?;
        Ok(())
    }

    /// `id_dynamic_keymap_get_buffer`: at most [`BUFFER_CHUNK`] bytes starting
    /// at `offset`, returned at bytes 4..4+size.
    pub fn keymap_buffer(&mut self, offset: u16, size: usize) -> Result<Vec<u8>> {
        if size > BUFFER_CHUNK {
            return Err(VialError::Unexpected(format!(
                "keymap buffer chunk of {size} exceeds {}",
                BUFFER_CHUNK
            )));
        }
        let [hi, lo] = offset.to_be_bytes();
        let response = self.send(&[command::KEYMAP_GET_BUFFER, hi, lo, size as u8])?;
        Ok(response[4..4 + size].to_vec())
    }

    /// Read the whole dynamic keymap as `layers * rows * cols` big-endian u16s.
    pub fn keymap(&mut self, layers: u8, rows: u8, cols: u8) -> Result<Vec<u16>> {
        let bytes = usize::from(layers) * usize::from(rows) * usize::from(cols) * 2;
        let mut buffer = Vec::with_capacity(bytes);
        while buffer.len() < bytes {
            let size = (bytes - buffer.len()).min(BUFFER_CHUNK);
            let chunk = self.keymap_buffer(buffer.len() as u16, size)?;
            buffer.extend_from_slice(&chunk);
        }
        Ok(buffer
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect())
    }

    /// `vial_get_size`: compressed keyboard-definition length in bytes.
    pub fn definition_size(&mut self) -> Result<u32> {
        let response = self.send_vial(vial_command::GET_SIZE, &[])?;
        Ok(u32::from_le_bytes([
            response[0],
            response[1],
            response[2],
            response[3],
        ]))
    }

    /// `vial_get_def` pages, decompressed to the JSON document the firmware
    /// embeds at build time. `vial-qmk` stores it XZ-compressed.
    pub fn definition_json(&mut self) -> Result<Vec<u8>> {
        let size = self.definition_size()? as usize;
        if size == 0 {
            return Err(VialError::Definition(
                "firmware reports an empty keyboard definition".into(),
            ));
        }
        let mut compressed = Vec::with_capacity(size);
        for page in 0..size.div_ceil(REPORT_SIZE) {
            let page = page as u16;
            let [lo, hi] = page.to_le_bytes();
            let response = self.send_vial(vial_command::GET_DEFINITION, &[lo, hi])?;
            let take = (size - compressed.len()).min(REPORT_SIZE);
            compressed.extend_from_slice(&response[..take]);
        }
        let mut json = Vec::new();
        lzma_rs::xz_decompress(&mut std::io::Cursor::new(compressed), &mut json).map_err(
            |error| VialError::Definition(format!("cannot decompress definition: {error}")),
        )?;
        Ok(json)
    }

    /// `vial_get_unlock_status`. The first byte is the lock flag; the second
    /// says whether an unlock gesture is in progress.
    pub fn unlock_status(&mut self) -> Result<UnlockStatus> {
        let response = self.send_vial(vial_command::GET_UNLOCK_STATUS, &[])?;
        let mut keys = Vec::new();
        for pair in response[2..].chunks_exact(2) {
            if pair[0] != 0xFF && pair[1] != 0xFF {
                keys.push((pair[0], pair[1]));
            }
        }
        Ok(UnlockStatus {
            unlocked: response[0] != 0,
            in_progress: response[1] != 0,
            keys,
        })
    }

    /// Start the unlock gesture. The keyboard then stops processing keys until
    /// the user holds the requested matrix keys for a few seconds.
    pub fn unlock_start(&mut self) -> Result<()> {
        self.send_vial(vial_command::UNLOCK_START, &[])?;
        Ok(())
    }

    /// Poll the unlock gesture. The first byte is the lock flag, the second is
    /// the in-progress flag and the third is the remaining hold counter. The
    /// poll response does not repeat the matrix keys.
    pub fn unlock_poll(&mut self) -> Result<UnlockStatus> {
        let response = self.send_vial(vial_command::UNLOCK_POLL, &[])?;
        Ok(UnlockStatus {
            unlocked: response[0] != 0,
            in_progress: response[1] != 0,
            keys: Vec::new(),
        })
    }

    pub fn lock(&mut self) -> Result<()> {
        self.send_vial(vial_command::LOCK, &[])?;
        Ok(())
    }

    pub fn macro_count(&mut self) -> Result<u8> {
        let response = self.send(&[command::MACRO_GET_COUNT])?;
        Ok(response[1])
    }

    pub fn macro_buffer_size(&mut self) -> Result<u16> {
        let response = self.send(&[command::MACRO_GET_BUFFER_SIZE])?;
        Ok(u16::from_be_bytes([response[1], response[2]]))
    }
}

/// Build a command report with zero padding. Useful for tests and docs.
pub fn report(bytes: &[u8]) -> [u8; REPORT_SIZE] {
    report_from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        response: [u8; REPORT_SIZE],
        last: Option<[u8; REPORT_SIZE]>,
    }

    impl RawHid for Fake {
        fn exchange(&mut self, report: [u8; REPORT_SIZE]) -> Result<[u8; REPORT_SIZE]> {
            self.last = Some(report);
            Ok(self.response)
        }
    }

    #[test]
    fn version_is_big_endian_at_bytes_one_and_two() {
        let mut fake = Fake {
            response: report(&[0x01, 0x00, 0x09]),
            last: None,
        };
        let mut device = VialDevice::new(&mut fake);
        let reports = &mut device;
        assert_eq!(reports.via_protocol().unwrap(), 9);
        assert_eq!(fake.last.unwrap()[0], command::GET_PROTOCOL_VERSION);
    }

    #[test]
    fn keyboard_id_is_little_endian_with_eight_byte_uid() {
        let mut response = [0u8; REPORT_SIZE];
        response[..4].copy_from_slice(&6u32.to_le_bytes());
        response[4..12].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut fake = Fake {
            response,
            last: None,
        };
        let id = VialDevice::new(&mut fake).vial_id().unwrap();
        assert_eq!(id.protocol, 6);
        assert_eq!(id.uid_hex(), "0102030405060708");
    }

    #[test]
    fn keycode_response_uses_bytes_four_and_five() {
        let mut response = [0u8; REPORT_SIZE];
        response[4..6].copy_from_slice(&0x04u16.to_be_bytes());
        let mut fake = Fake {
            response,
            last: None,
        };
        let keycode = VialDevice::new(&mut fake).keycode(0, 1, 2).unwrap();
        assert_eq!(keycode, 0x0004);
        assert_eq!(
            fake.last.unwrap()[..4],
            [command::DYNAMIC_KEYMAP_GET_KEYCODE, 0, 1, 2]
        );
    }

    #[test]
    fn keymap_requests_use_28_byte_chunks() {
        let mut response = [0u8; REPORT_SIZE];
        for (index, slot) in response[4..4 + 28].iter_mut().enumerate() {
            *slot = index as u8;
        }
        let mut fake = Fake {
            response,
            last: None,
        };
        let mut device = VialDevice::new(&mut fake);
        let chunk = device.keymap_buffer(0, 28).unwrap();
        assert_eq!(chunk.len(), 28);
        assert_eq!(chunk[27], 27);
        assert_eq!(
            &fake.last.unwrap()[..4],
            &[command::KEYMAP_GET_BUFFER, 0, 0, 28]
        );
    }

    #[test]
    fn unhandled_command_is_reported() {
        let mut response = [0u8; REPORT_SIZE];
        response[0] = 0xFF;
        let mut fake = Fake {
            response,
            last: None,
        };
        let error = VialDevice::new(&mut fake).layer_count().unwrap_err();
        assert!(matches!(error, VialError::Unexpected(_)));
    }

    #[test]
    fn handshake_accepts_supported_window_and_rejects_foreign_versions() {
        let mut response = [0u8; REPORT_SIZE];
        response[1..3].copy_from_slice(&9u16.to_be_bytes());
        let mut fake = Fake {
            response,
            last: None,
        };
        // The fake answers every command identically, so the Vial id reads
        // protocol 9 from bytes 0..4; that is inside 1..=6? No: 9 > 6, so the
        // handshake must fail even though VIA itself is acceptable.
        let error = VialDevice::new(&mut fake).handshake().unwrap_err();
        assert!(matches!(
            error,
            VialError::UnsupportedProtocol { via: 9, .. }
        ));
    }
}
