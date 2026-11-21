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
