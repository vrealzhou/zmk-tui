//! Vial / VIA keyboard support.
//!
//! This module adds a second device backend to the editor without touching the
//! ZMK Studio path. It speaks the Vial raw-HID protocol (32-byte reports on HID
//! usage page `0xFF60`), reads the keyboard definition the firmware embeds,
//! and projects the QMK keymap onto the editor's existing domain model.
//!
//! Layering:
//!
//! ```text
//! TUI (unchanged)
//!   └─ VialKeyboard          session.rs  — staging, save/discard, capability flags
//!       └─ VialDevice        protocol.rs — command/report codec
//!           └─ RawHid         transport.rs (USB hidapi) / simulate.rs (tests)
//! ```
//!
//! The only semantic bridge to the UI is the pseudo behavior catalog in
//! [`keycode`]. Every 16-bit QMK keycode decodes to a binding and encodes back
//! exactly, so unsupported features (tap dance, macros, custom keycodes) are
//! preserved rather than rewritten.
//!
//! Not supported by this module: macros, tap dance, combos, key overrides,
//! QMK settings, encoders, lighting and physical-layout switching. Those are
//! separate Vial protocol entries; the editor must not pretend to edit them.

pub mod client;
pub mod definition;
pub mod keycode;
pub mod protocol;
pub mod session;
pub mod simulate;
pub mod transport;
pub mod vil;

pub use client::VialClient;
pub use definition::{KeyPosition, KeyboardDefinition, Matrix};
pub use keycode::{Dialect, Translator};
pub use protocol::{
    BUFFER_CHUNK, HID_USAGE, HID_USAGE_PAGE, ProtocolInfo, REPORT_SIZE, RawHid, SERIAL_MAGIC,
    UnlockStatus, VIA_PROTOCOL_VERSION, VIAL_PROTOCOL_MAX, VialDevice, VialError, VialId,
};
pub use session::{CAPABILITIES, Capabilities, VialKeyboard, VialSnapshot};
pub use simulate::SimulatedVial;
