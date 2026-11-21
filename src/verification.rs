//! Kani proof harnesses for protocol invariants.
//!
//! Run with `cargo kani --harness <name>`; this module is compiled only under
//! Kani's `kani` cfg and is absent from normal builds and tests.

use crate::protocol::frame::{Decoder, EOF, ESC, SOF, encode};

/// Every bounded payload survives encode → incremental decode unchanged,
/// including payloads made entirely of reserved delimiter bytes.
#[kani::proof]
fn frame_round_trips_arbitrary_payload() {
    let payload: [u8; 8] = kani::any();
    let encoded = encode(&payload);
    let mut decoder = Decoder::new();
    let mut frames = Vec::new();
    decoder
        .feed_slice(&encoded, &mut frames)
        .expect("bounded payload cannot exceed the frame limit");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], payload);
}

/// Arbitrary serial noise must not panic, overflow, or allocate unboundedly
/// in the decoder. The input length is fixed so the proof is finite.
#[kani::proof]
fn arbitrary_serial_noise_never_panics() {
    let bytes: [u8; 16] = kani::any();
    let mut decoder = Decoder::new();
    let _ = decoder.decode(&bytes);
}

/// Every encoded frame is delimited and every reserved byte in the body is
/// escaped. This avoids the decoder and is the cheapest proof obligation.
#[kani::proof]
fn encoded_frame_shape_is_well_formed() {
    let payload: [u8; 4] = kani::any();
    let encoded = encode(&payload);
    assert_eq!(encoded.first(), Some(&SOF));
    assert_eq!(encoded.last(), Some(&EOF));

    let body = &encoded[1..encoded.len() - 1];
    let mut escaped = false;
    for &byte in body {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == ESC {
            escaped = true;
            continue;
        }
        assert!(byte != SOF && byte != EOF, "unescaped delimiter in body");
    }
    assert!(!escaped, "dangling escape at end of frame");
}

/// A fast, allocation-light proof: every reserved byte is escaped when it is
/// the entire payload. This is the proof used by the quality gate; the larger
/// round-trip harnesses remain available for a dedicated verification job.
#[kani::proof]
fn encode_escapes_each_reserved_single_byte() {
    let byte: u8 = kani::any();
    let encoded = encode(&[byte]);
    match byte {
        SOF | ESC | EOF => {
            assert_eq!(encoded.len(), 4);
            assert_eq!(encoded[0], SOF);
            assert_eq!(encoded[1], ESC);
            assert_eq!(encoded[2], byte);
            assert_eq!(encoded[3], EOF);
        }
        _ => {
            assert_eq!(encoded.len(), 3);
            assert_eq!(encoded[0], SOF);
            assert_eq!(encoded[1], byte);
            assert_eq!(encoded[2], EOF);
        }
    }
}
