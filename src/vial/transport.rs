//! Real USB transport for Vial keyboards.
//!
//! Vial uses a vendor-defined HID interface with usage page `0xFF60` and usage
//! `0x61`; each exchange is a 32-byte output report followed by a 32-byte input
//! report. `hidapi` writes the report-ID-0 prefix itself only if it is part of
//! the buffer, so the buffer passed to `write` is `[0, ..32 bytes]`.
//!
//! The interface and serial string are descriptors only: enumeration never
//! opens the device, so scanning cannot disturb a keyboard that another
//! program (Vial GUI, a browser using WebHID) currently owns.

use hidapi::{HidApi, HidDevice};

use crate::vial::protocol::{
    HID_USAGE, HID_USAGE_PAGE, REPORT_SIZE, RawHid, Result, SERIAL_MAGIC, VIAL_PROTOCOL_MAX,
    VialDevice, VialError, VialId,
};

/// A short description of a discovered raw-HID interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidDescriptor {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub product: String,
    pub path: String,
    pub usage_page: u16,
    pub usage: u16,
    pub is_vial: bool,
}

/// A Vial keyboard discovered on the raw-HID interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VialDeviceInfo {
    pub name: String,
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
}

impl VialDeviceInfo {
    /// Label shown in the connect list.
    pub fn label(&self) -> String {
        let name = if self.name.trim().is_empty() {
            "Vial keyboard"
        } else {
            self.name.trim()
        };
        format!("{name} (Vial)")
    }
}

/// Enumerate every HID interface. Used by diagnostics to see what a 2.4G
/// receiver actually exposes (a plain keyboard endpoint has no Vial protocol).
pub fn list_all() -> Result<Vec<HidDescriptor>> {
    let api = HidApi::new().map_err(|error| VialError::Transport(error.to_string()))?;
    Ok(api
        .device_list()
        .map(|device| {
            let serial = device.serial_number().unwrap_or_default().to_string();
            HidDescriptor {
                vendor_id: device.vendor_id(),
                product_id: device.product_id(),
                is_vial: serial.contains(SERIAL_MAGIC),
                serial,
                product: device.product_string().unwrap_or_default().to_string(),
                path: device.path().to_string_lossy().into_owned(),
                usage_page: device.usage_page(),
                usage: device.usage(),
            }
        })
        .collect())
}

/// Enumerate the raw-HID interfaces that look like Vial or VIA keyboards.
pub fn list() -> Result<Vec<HidDescriptor>> {
    Ok(list_all()?
        .into_iter()
        .filter(|device| device.usage_page == HID_USAGE_PAGE && device.usage == HID_USAGE)
        .collect())
}

/// A protocol response counts as Vial when the version is in the supported
/// range and the UID is non-zero. A plain VIA keyboard echoes/unhandles the
/// command, producing 0xFE/0xFF values that fail this check.
fn looks_like_vial(id: &VialId) -> bool {
    (1..=VIAL_PROTOCOL_MAX).contains(&id.protocol) && id.uid != [0u8; 8]
}

fn probe_vial(path: &str) -> bool {
    let Ok(mut transport) = open_path(path) else {
        return false;
    };
    VialDevice::new(&mut transport)
        .vial_id()
        .is_ok_and(|id| looks_like_vial(&id))
}

/// Enumerate Vial keyboards. Builds made by `build_vial.mk` carry
/// `SERIAL_NUMBER="vial:f64c2b3c"`, which is enough to classify them without
/// opening the device. Some 2.4G receivers and third-party builds do not
/// forward that serial string, so interfaces without it get one read-only
/// `vial_get_keyboard_id` probe as a fallback.
pub fn list_vial_devices() -> Vec<VialDeviceInfo> {
    let Ok(descriptors) = list() else {
        return Vec::new();
    };
    descriptors
        .into_iter()
        .filter(|descriptor| descriptor.is_vial || probe_vial(&descriptor.path))
        .map(|descriptor| VialDeviceInfo {
            name: if descriptor.product.is_empty() {
                "Vial keyboard".into()
            } else {
                descriptor.product
            },
            path: descriptor.path,
            vendor_id: descriptor.vendor_id,
            product_id: descriptor.product_id,
            serial: descriptor.serial,
        })
        .collect()
}

/// Open one specific raw-HID interface by its platform path.
pub fn open_path(path: &str) -> Result<VialHid> {
    let api = HidApi::new().map_err(|error| VialError::Transport(error.to_string()))?;
    let descriptor = list()?
        .into_iter()
        .find(|descriptor| descriptor.path == path)
        .ok_or_else(|| VialError::Transport(format!("no raw-HID interface at path {path}")))?;
    VialHid::open_descriptor(&api, descriptor)
}

pub struct VialHid {
    device: HidDevice,
    descriptor: HidDescriptor,
}

impl VialHid {
    /// Open the first interface whose serial number carries Vial's magic, or
    /// failing that, the first raw-HID candidate.
    pub fn open_first() -> Result<Self> {
        let descriptors = list()?;
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.is_vial)
            .or_else(|| descriptors.first())
            .cloned()
            .ok_or_else(|| {
                VialError::Transport(
                    "no Vial/VIA raw-HID interface found (usage page 0xFF60, usage 0x61)".into(),
                )
            })?;
        let api = HidApi::new().map_err(|error| VialError::Transport(error.to_string()))?;
        Self::open_descriptor(&api, descriptor)
    }

    fn open_descriptor(api: &HidApi, descriptor: HidDescriptor) -> Result<Self> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_supported_nonzero_vial_ids_are_accepted() {
        let uid = *b"VIALDEMO";
        assert!(looks_like_vial(&VialId { protocol: 6, uid }));
        assert!(!looks_like_vial(&VialId { protocol: 0, uid }));
        assert!(!looks_like_vial(&VialId { protocol: 7, uid }));
        assert!(!looks_like_vial(&VialId {
            protocol: 6,
            uid: [0; 8]
        }));
    }
}
