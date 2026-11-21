//! Message framing for the ZMK Studio RPC protocol.
//!
//! A frame is a start-of-frame byte, a payload with the three reserved bytes
//! escaped, and an end-of-frame byte:
//!
//! | byte | meaning |
//! |------|---------|
//! | `0xAB` | start of frame |
//! | `0xAC` | escape; the next byte is literal payload |
//! | `0xAD` | end of frame |
//!
//! Escaping prefixes every reserved byte with `0xAC`, including `0xAC` itself.

use thiserror::Error;

pub const SOF: u8 = 0xAB;
pub const ESC: u8 = 0xAC;
pub const EOF: u8 = 0xAD;

/// Frames larger than this are treated as corrupt. Keymaps are a few KiB at
/// most, so this is generous while still bounding memory use.
pub const MAX_FRAME_SIZE: usize = 256 * 1024;

/// Encode a payload into one framed message.
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.push(SOF);
    for &byte in payload {
        if matches!(byte, SOF | ESC | EOF) {
            out.push(ESC);
        }
        out.push(byte);
    }
    out.push(EOF);
    out
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame exceeded the maximum size of {MAX_FRAME_SIZE} bytes")]
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    InFrame,
    Escaped,
}

/// Incremental decoder for the framed protocol.
///
/// The decoder is resilient to serial noise: bytes outside a frame are
/// ignored, and a start-of-frame byte received mid-frame restarts the frame
/// instead of failing.
#[derive(Debug)]
pub struct Decoder {
    state: State,
    buffer: Vec<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            buffer: Vec::with_capacity(1024),
        }
    }

    /// Feed one byte. Returns a completed frame, if any.
    pub fn feed(&mut self, byte: u8) -> Result<Option<Vec<u8>>, FrameError> {
        match self.state {
            State::Idle => {
                if byte == SOF {
                    self.state = State::InFrame;
                    self.buffer.clear();
                }
                Ok(None)
            }
            State::InFrame => match byte {
                SOF => {
                    // Restart rather than emit a truncated frame.
                    self.buffer.clear();
                    Ok(None)
                }
                ESC => {
                    self.state = State::Escaped;
                    Ok(None)
                }
                EOF => {
                    self.state = State::Idle;
                    Ok(Some(std::mem::take(&mut self.buffer)))
                }
                _ => {
                    self.push(byte)?;
                    Ok(None)
                }
            },
            State::Escaped => {
                self.state = State::InFrame;
                self.push(byte)?;
                Ok(None)
            }
        }
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameError> {
        if self.buffer.len() >= MAX_FRAME_SIZE {
            self.state = State::Idle;
            self.buffer.clear();
            return Err(FrameError::TooLarge);
        }
        self.buffer.push(byte);
        Ok(())
    }

    /// Feed a chunk of bytes, appending completed frames to `out`.
    pub fn feed_slice(&mut self, bytes: &[u8], out: &mut Vec<Vec<u8>>) -> Result<(), FrameError> {
        for &byte in bytes {
            if let Some(frame) = self.feed(byte)? {
                out.push(frame);
            }
        }
        Ok(())
    }

    /// Feed a chunk and return all completed frames.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        let mut frames = Vec::new();
        self.feed_slice(bytes, &mut frames)?;
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payloads() -> Vec<Vec<u8>> {
        vec![
            vec![],
            vec![0x01, 0x02, 0x03],
            vec![SOF],
            vec![ESC],
            vec![EOF],
            vec![SOF, ESC, EOF],
            vec![0x00, SOF, ESC, ESC, EOF, 0xFF],
            (0..=255u8).collect(),
        ]
    }

    #[test]
    fn round_trips_every_byte_value() {
        let decoder = &mut Decoder::new();
        for payload in payloads() {
            let encoded = encode(&payload);
            assert_eq!(encoded.first(), Some(&SOF));
            assert_eq!(encoded.last(), Some(&EOF));
            let frames = decoder.decode(&encoded).expect("valid frame");
            assert_eq!(frames, vec![payload.clone()]);
        }
    }

    #[test]
    fn decodes_when_fed_one_byte_at_a_time() {
        let payload = vec![SOF, 0x10, ESC, EOF, ESC];
        let encoded = encode(&payload);
        let mut decoder = Decoder::new();
        let mut frames = Vec::new();
        for byte in encoded {
            if let Some(frame) = decoder.feed(byte).expect("valid frame") {
                frames.push(frame);
            }
        }
        assert_eq!(frames, vec![payload]);
    }

    #[test]
    fn decodes_multiple_frames_in_one_chunk() {
        let mut bytes = encode(&[1, 2]);
        bytes.extend(encode(&[3, SOF, 4]));
        let frames = Decoder::new().decode(&bytes).expect("valid frames");
        assert_eq!(frames, vec![vec![1, 2], vec![3, SOF, 4]]);
    }

    #[test]
    fn ignores_noise_between_frames() {
        let mut bytes = vec![0x00, 0xFF, 0x12];
        bytes.extend(encode(&[9, 9]));
        bytes.extend([0x55, 0x66]);
        bytes.extend(encode(&[7]));
        let frames = Decoder::new().decode(&bytes).expect("valid frames");
        assert_eq!(frames, vec![vec![9, 9], vec![7]]);
    }

    #[test]
    fn restarts_on_start_of_frame_mid_frame() {
        let mut bytes = vec![SOF, 1, 2, 3, SOF, 9];
        bytes.push(EOF);
        let frames = Decoder::new().decode(&bytes).expect("valid frame");
        assert_eq!(frames, vec![vec![9]]);
    }

    #[test]
    fn escaped_end_of_frame_is_payload() {
        let payload = [ESC, EOF, ESC, EOF];
        let encoded = encode(&payload);
        assert_eq!(
            encoded,
            vec![SOF, ESC, ESC, ESC, EOF, ESC, ESC, ESC, EOF, EOF]
        );
        let frames = Decoder::new().decode(&encoded).expect("valid frame");
        assert_eq!(frames, vec![payload.to_vec()]);
    }

    #[test]
    fn empty_frame_is_emitted() {
        let frames = Decoder::new().decode(&[SOF, EOF]).expect("valid frame");
        assert_eq!(frames, vec![Vec::<u8>::new()]);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut decoder = Decoder::new();
        decoder.feed(SOF).unwrap();
        for _ in 0..MAX_FRAME_SIZE {
            decoder.feed(0x01).unwrap();
        }
        assert_eq!(decoder.feed(0x01), Err(FrameError::TooLarge));
        // The decoder recovers and can decode the next frame.
        let frames = decoder.decode(&encode(&[1, 2, 3])).expect("valid frame");
        assert_eq!(frames, vec![vec![1, 2, 3]]);
    }
}
