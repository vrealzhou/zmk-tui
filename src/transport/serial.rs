//! USB transport: ZMK Studio speaks the framed RPC protocol over a CDC-ACM
//! serial port (usually shown as `/dev/cu.usbmodem…` on macOS).

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serialport::SerialPort;

use super::{Connection, ConnectionKind, FrameResult, Sink};
use crate::protocol::frame::Decoder;

const BAUD_RATE: u32 = 9600;
const READ_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub path: String,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
}

impl UsbDevice {
    /// A human-readable label, including the port path so duplicate products
    /// remain distinguishable.
    pub fn label(&self) -> String {
        match self.product.as_deref() {
            Some(product) if !product.is_empty() => format!("{product}  {}", self.path),
            _ => self.path.clone(),
        }
    }
}

/// List USB serial ports. Non-USB ports are hidden because a ZMK keyboard
/// always enumerates as a USB device for the Studio USB transport.
pub fn list_usb_devices() -> Vec<UsbDevice> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };

    let mut devices: Vec<UsbDevice> = ports
        .into_iter()
        .filter(|port| !is_tty_twin(&port.port_name))
        .filter_map(|port| match port.port_type {
            serialport::SerialPortType::UsbPort(info) => Some(UsbDevice {
                path: port.port_name,
                product: info.product,
                manufacturer: info.manufacturer,
                serial_number: info.serial_number,
            }),
            _ => None,
        })
        .collect();

    devices.sort_by(|a, b| a.path.cmp(&b.path));
    devices
}

/// macOS exposes both `/dev/cu.*` (callout, correct for outgoing connections)
/// and `/dev/tty.*` for the same device. Keep only the callout device.
fn is_tty_twin(path: &str) -> bool {
    path.contains("/dev/tty.")
}

/// Open a USB serial connection and start reader/writer threads.
pub fn connect(path: &str) -> Result<Connection> {
    let port = serialport::new(path, BAUD_RATE)
        .timeout(READ_TIMEOUT)
        .open()
        .map_err(|error| open_error(path, &error.to_string()))?;

    let reader_port = port
        .try_clone()
        .with_context(|| format!("could not open a second handle for {path}"))?;

    let (frames_tx, frames) = tokio::sync::mpsc::unbounded_channel::<FrameResult>();
    let (write_tx, write_rx) = std_mpsc::channel::<Vec<u8>>();
    let shutdown = Arc::new(AtomicBool::new(false));

    spawn_reader(reader_port, frames_tx.clone(), shutdown.clone());
    spawn_writer(port, write_rx, frames_tx);

    Ok(Connection {
        label: path.to_string(),
        kind: ConnectionKind::Usb,
        sink: Sink::Serial(SerialSink {
            writer: write_tx,
            shutdown,
        }),
        frames,
    })
}

fn spawn_reader(
    mut port: Box<dyn SerialPort>,
    frames_tx: tokio::sync::mpsc::UnboundedSender<FrameResult>,
    shutdown: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("zmk-usb-read".into())
        .spawn(move || {
            let mut buffer = [0u8; 512];
            let mut decoder = Decoder::new();
            let mut completed = Vec::new();

            loop {
                // The shutdown flag is set when the client is dropped, so the
                // port is released even if the peer never sends another byte.
                if shutdown.load(Ordering::Relaxed) || frames_tx.is_closed() {
                    break;
                }
                match port.read(&mut buffer) {
                    Ok(0) => std::thread::sleep(Duration::from_millis(10)),
                    Ok(n) => {
                        completed.clear();
                        let result = decoder.feed_slice(&buffer[..n], &mut completed);
                        // Frames that completed before a corrupt byte run are
                        // still valid and must not be dropped.
                        for frame in completed.drain(..) {
                            if frames_tx.send(Ok(frame)).is_err() {
                                return;
                            }
                        }
                        if result.is_err() {
                            // Corrupt stream: the decoder has resynchronised.
                            continue;
                        }
                    }
                    Err(error) if is_timeout(&error) => continue,
                    Err(error) => {
                        let _ = frames_tx.send(Err(format!("USB read failed: {error}")));
                        return;
                    }
                }
            }
        })
        .expect("spawn USB reader thread");
}

fn spawn_writer(
    mut port: Box<dyn SerialPort>,
    write_rx: std_mpsc::Receiver<Vec<u8>>,
    frames_tx: tokio::sync::mpsc::UnboundedSender<FrameResult>,
) {
    std::thread::Builder::new()
        .name("zmk-usb-write".into())
        .spawn(move || {
            while let Ok(bytes) = write_rx.recv() {
                if let Err(error) = port.write_all(&bytes).and_then(|()| port.flush()) {
                    let _ = frames_tx.send(Err(format!("USB write failed: {error}")));
                    return;
                }
            }
        })
        .expect("spawn USB writer thread");
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

pub struct SerialSink {
    writer: std_mpsc::Sender<Vec<u8>>,
    /// Signals the reader thread to stop and release the serial port.
    shutdown: Arc<AtomicBool>,
}

impl Drop for SerialSink {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl SerialSink {
    pub fn send(&self, framed: Vec<u8>) -> Result<()> {
        self.writer
            .send(framed)
            .map_err(|_| anyhow!("USB connection is closed"))
    }
}

/// Turn a low-level open failure into an actionable message. EBUSY almost
/// always means ZMK Studio (web or native), a serial monitor, or another
/// zmk-tui instance already owns the Studio endpoint.
fn open_error(path: &str, detail: &str) -> anyhow::Error {
    if is_busy_error(detail) {
        anyhow!("{path} is busy — close ZMK Studio or another app using the port, then retry")
    } else {
        anyhow!("could not open serial port {path}: {detail}")
    }
}

fn is_busy_error(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    [
        "busy",
        "in use",
        "being used by another process",
        "sharing violation",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_port_in_use_errors() {
        for detail in [
            "Device or resource busy",
            "Resource busy",
            "The process cannot access the file because it is being used by another process",
            "sharing violation",
            "resource in use",
        ] {
            assert!(is_busy_error(detail), "should be busy: {detail}");
        }
        assert!(!is_busy_error("Permission denied"));
        assert!(!is_busy_error("No such file or directory"));
    }

    #[test]
    fn busy_error_is_actionable() {
        let message = open_error("/dev/cu.usbmodem2101", "Device or resource busy").to_string();
        assert!(message.contains("/dev/cu.usbmodem2101"), "{message}");
        assert!(message.contains("ZMK Studio"), "{message}");
        assert!(message.contains("retry"), "{message}");
    }

    #[test]
    fn other_errors_keep_the_os_detail() {
        let message = open_error("/dev/cu.usbmodem2101", "Permission denied").to_string();
        assert!(message.contains("Permission denied"), "{message}");
        assert!(!message.contains("ZMK Studio"), "{message}");
    }
}
