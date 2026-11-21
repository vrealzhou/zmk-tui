//! Terminal UI for editing ZMK keymaps over USB or Bluetooth.
//!
//! The crate is split into a transport-agnostic ZMK Studio client
//! ([`client`]), a UI layer ([`tui`]), and small domain modules shared by both.

pub mod client;
pub mod device;
pub mod keycodes;
pub mod labels;
pub mod layout;
pub mod model;
pub mod protocol;
pub mod simulator;
pub mod transport;
pub mod tui;
pub mod vial;

#[cfg(kani)]
mod verification;
