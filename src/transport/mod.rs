//! Framed byte transports: USB CDC-ACM serial and Bluetooth LE GATT.

pub mod ble;
pub mod serial;

use tokio::sync::mpsc;

use crate::protocol::frame::encode as encode_frame;

/// A transport-level failure. Errors are strings because they cross a task
/// boundary into the UI and only need to be displayed.
pub type FrameResult = Result<Vec<u8>, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Usb,
    Bluetooth,
    /// In-process channel, used by tests and the built-in simulator.
    Channel,
}

impl ConnectionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Bluetooth => "Bluetooth",
            Self::Channel => "Simulator",
        }
    }
}

pub enum Sink {
    Serial(serial::SerialSink),
    Bluetooth(ble::BleSink),
    Channel(mpsc::UnboundedSender<Vec<u8>>),
}

impl Sink {
    /// Frame a protobuf payload and send it.
    pub async fn send(&self, payload: Vec<u8>) -> anyhow::Result<()> {
        let framed = encode_frame(&payload);
        match self {
            Self::Serial(sink) => sink.send(framed),
            Self::Bluetooth(sink) => sink.send(framed).await,
            Self::Channel(sender) => sender
                .send(framed)
                .map_err(|_| anyhow::anyhow!("simulator channel closed")),
        }
    }
}

/// An established byte-stream connection to a keyboard.
pub struct Connection {
    pub label: String,
    pub kind: ConnectionKind,
    pub sink: Sink,
    pub frames: mpsc::UnboundedReceiver<FrameResult>,
}

impl Connection {
    /// Build an in-process connection for tests and the simulator.
    ///
    /// Returns the connection plus the two ends used by the fake device:
    /// framed bytes written by the client, and a channel for sending framed
    /// payloads back to the client.
    pub fn duplex(
        label: impl Into<String>,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<Vec<u8>>,
        mpsc::UnboundedSender<FrameResult>,
    ) {
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        (
            Self {
                label: label.into(),
                kind: ConnectionKind::Channel,
                sink: Sink::Channel(outgoing_tx),
                frames: incoming_rx,
            },
            outgoing_rx,
            incoming_tx,
        )
    }
}
