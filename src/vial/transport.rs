//! Real USB transport for Vial keyboards.
//!
//! Vial uses a vendor-defined HID interface with usage page `0xFF60` and usage
//! `0x61`; each exchange is a 32-byte output report followed by a 32-byte input
//! report. `hidapi` writes the report-ID-0 prefix itself only if it is part of
//! the buffer, so the buffer passed to `write` is `[0, ..32 bytes]`.
//!
//! This module is behind the `vial-hid` feature so the default ZMK Studio
//! build does not link a USB HID stack.

#![cfg(feature = "vial-hid")]

use hidapi::{HidApi, HidDevice};

use crate::vial::protocol::{
    HID_USAGE, HID_USAGE_PAGE, REPORT_SIZE, RawHid, Result, SERIAL_MAGIC, VialError,
};

/// A short description of a discovered raw-HID interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidDescriptor {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub product: String,
    pub path: String,
    pub is_vial: bool,
}

/// Enumerate the raw-HID interfaces that look like Vial or VIA keyboards.
pub fn list() -> Result<Vec<HidDescriptor>> {
    let api = HidApi::new().map_err(|error| VialError::Transport(error.to_string()))?;
    Ok(api
        .device_list()
        .filter(|device| device.usage_page() == HID_USAGE_PAGE && device.usage() == HID_USAGE)
        .map(|device| {
            let serial = device.serial_number().unwrap_or_default().to_string();
            HidDescriptor {
                vendor_id: device.vendor_id(),
                product_id: device.product_id(),
                is_vial: serial.contains(SERIAL_MAGIC),
                serial,
                product: device.product_string().unwrap_or_default().to_string(),
                path: device.path().to_string_lossy().into_owned(),
            }
        })
        .collect())
}

pub struct VialHid {
    device: HidDevice,
    descriptor: HidDescriptor,
}

impl VialHid {
    /// Open the first interface whose serial number carries Vial's magic, or
    /// failing that, the first raw-HID candidate.
    pub fn open_first() -> Result<Self> {
        let api = HidApi::new().map_err(|error| VialError::Transport(error.to_string()))?;
        let mut vial = None;
        let mut fallback = None;
        for device in api.device_list() {
            if device.usage_page() != HID_USAGE_PAGE || device.usage() != HID_USAGE {
                continue;
            }
            let serial = device.serial_number().unwrap_or_default().to_string();
            let descriptor = HidDescriptor {
                vendor_id: device.vendor_id(),
                product_id: device.product_id(),
                is_vial: serial.contains(SERIAL_MAGIC),
                serial,
                product: device.product_string().unwrap_or_default().to_string(),
                path: device.path().to_string_lossy().into_owned(),
            };
            if descriptor.is_vial {
                vial = Some(descriptor);
                break;
            }
            fallback.get_or_insert(descriptor);
        }
        let descriptor = vial.or(fallback).ok_or_else(|| {
            VialError::Transport(
                "no Vial/VIA raw-HID interface found (usage page 0xFF60, usage 0x61)".into(),
            )
        })?;
        let path = std::ffi::CString::new(descriptor.path.as_bytes())
            .map_err(|error| VialError::Transport(format!("invalid HID path: {error}")))?;
        let device = api
            .open_path(&path)
            .map_err(|error| VialError::Transport(error.to_string()))?;
        Ok(Self { device, descriptor })
    }

    pub fn descriptor(&self) -> &HidDescriptor {
        &self.descriptor
    }
}

impl RawHid for VialHid {
    fn exchange(&mut self, request: [u8; REPORT_SIZE]) -> Result<[u8; REPORT_SIZE]> {
        let mut output = [0u8; REPORT_SIZE + 1];
        output[1..].copy_from_slice(&request);
        let written = self
            .device
            .write(&output)
            .map_err(|error| VialError::Transport(error.to_string()))?;
        if written != REPORT_SIZE + 1 {
            return Err(VialError::Transport(format!(
                "short HID write: {written} of {} bytes",
                REPORT_SIZE + 1
            )));
        }
        let mut response = [0u8; REPORT_SIZE];
        let read = self
            .device
            .read_timeout(&mut response, 500)
            .map_err(|error| VialError::Transport(error.to_string()))?;
        if read == 0 {
            return Err(VialError::Timeout);
        }
        Ok(response)
    }
}
