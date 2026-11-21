# Formal verification assessment

This document maps the codebase to properties that are worth proving and to
tools that can actually prove them. It is intentionally conservative: a Kani
harness is a bounded proof, not a claim that the whole application is verified.

## Summary

| Component | Candidate properties | Tool | Suitability | Current artefacts |
| --- | --- | --- | --- | --- |
| `protocol::frame` | encode/decode round-trip for all payload bytes; arbitrary serial noise never panics; frame size bound | Kani | **High** | `src/verification.rs` harnesses |
| `labels`, `keycodes` | total functions; label/name length bounds; stable search ordering | Kani or Prusti contracts | Medium | unit/property tests; no proofs yet |
| `layout` | selection remains in range; visible cell invariants; no division by zero | Kani for integer paths, proptest for `f32` geometry | Low–medium | unit tests + screenshots |
| `client` | at most one in-flight mutation; request-id correlation; timeout/retry never confuses responses | TLA+ or Stateright | **Medium–high** | integration tests |
| `simulator` | lock/save/discard/layer state transitions preserve storage invariants | TLA+ or Stateright | Medium | TUI scenarios |
| `tui::app`, `modals` | modal exclusivity; dirty-edit confirmation; layer selection bounds | model-based/state-machine testing | Low | unit + Gherkin tests |
| `transport::ble`, `transport::serial` | OS/FFI handles stay valid; disconnect/reconnect cleanup | not suitable for deductive proof | — | hardware tests, debug logs |

## High-value target: frame codec

`src/protocol/frame.rs` is the best formal-verification target because it is
small, synchronous, allocation-bounded, and has a clean mathematical contract:

1. `decode(encode(p)) == [p]` for every payload up to the frame limit.
2. Reserved bytes (`SOF`, `ESC`, `EOF`) are escaped exactly once.
3. Arbitrary serial noise never panics, overflows, or grows the buffer past
   `MAX_FRAME_SIZE`.
4. A truncated frame is never emitted; a new `SOF` restarts the frame.

The Kani harnesses in `src/verification.rs` are set up to prove (1) for all
8-byte payloads, (3) for all 16-byte inputs, and a smaller encode-shape
invariant over four-byte payloads. They are bounded because Kani explores the
complete input space for a fixed length; unbounded payload length needs a loop
invariant or a length parameter with a carefully bounded `kani::assume`.

Status: `cargo kani --only-codegen` succeeds for the harness crate (Kani 0.67,
Rust 1.99), and the quality gate runs
`encode_escapes_each_reserved_single_byte` to completion:
`VERIFICATION SUCCESSFUL` in ~1.4s (418 checks). The larger
`frame_round_trips_arbitrary_payload` and
`arbitrary_serial_noise_never_panics` harnesses use `Vec`/`Vec<Vec<u8>>`; their
full CBMC runs exceed the local budget and remain proof obligations for a
dedicated verification job. Treat those two as bounded proof starting points,
not completed proofs, until that job reports `VERIFICATION SUCCESSFUL`.

Run them with:

```bash
cargo kani --harness frame_round_trips_arbitrary_payload
cargo kani --harness arbitrary_serial_noise_never_panics
cargo kani --harness encoded_frame_shape_is_well_formed
```

Kani can be slow on allocation-heavy code. If a harness exceeds the budget,
split it into a pure `decode_step(state, byte)` function and prove the step
function, then prove the loop by induction. A simpler next harness is an
`encode`-shape proof over a four-byte symbolic payload, which avoids decoding
and should discharge quickly.

## Function-level shortlist

| Function(s) | Property worth proving | Tool |
| --- | --- | --- |
| `protocol::frame::encode`, `Decoder::feed/feed_slice` | round-trip, escaping, no panic, bounded buffer | Kani (started) |
| `labels::short_behavior_name`, `binding_label` | total; short label length ≤ 5; no panic for catalog entries | Kani/Prusti |
| `keycodes::modifier_symbol_for_keycode`, `search` | total; `Some` only for bare modifiers; no panic for any query | Kani/Prusti |
| `layout::move_selection`, `position_at` | result in range; no panic for arbitrary cells | Kani/Prusti |
| `client::read_request`, mutation send path | response belongs to the request; mutations never auto-retry | TLA+/Stateright |
| `simulator` command/state handlers | save/discard/lock transitions preserve invariants | TLA+/Stateright |
| `tui::modals::BindingWizard` | stage transitions cannot reach Review with an invalid binding; filters always clamp selections | Stateright/model-based |
| `tui::mouse::MouseMap::hit` | hit-tests stay within recorded rects; list precedence is deterministic | Kani/property tests |

The first row is implemented; the others are recommended next steps. Kani is a
good fit for the small pure functions, while the client/simulator need an
explicit state-machine model rather than symbolic execution of async I/O.

## Other candidates

### Prusti / Creusot contracts

Useful for pure functions with simple pre/postconditions:

- `labels::short_behavior_name`: result is non-empty only for supported names,
  and is at most five characters.
- `layout::position_at`: a returned position is always `< grid.cells.len()`.
- `model::Keymap::layer_index_by_id`: returned index is always `< layers.len()`.
- `keycodes::modifier_symbol_for_keycode`: result is `Some` iff the value is a
  bare left/right modifier.

These are cheap contracts but low risk; Kani/property tests may be sufficient.

### TLA+ / Stateright for the client and simulator

The RPC client has an interesting concurrent protocol: one request id, an
in-flight mutation, read retries, disconnect events, and a lock-polling task.
The important safety property is that a response is delivered to the request
that produced it and that a mutation is never retried automatically. A small
TLA+ spec or a Stateright model with actions `send`, `receive`, `timeout`,
`disconnect`, `retry_read` would find correlation bugs that unit tests miss.

The simulator is another finite state machine: layers/bindings, dirty and saved
copies, lock state, and save/discard. Model checking the transitions would
strengthen the Gherkin acceptance tests.

### Why not the whole TUI?

Rendering and modal code depend on ratatui buffers, `ListState`, terminal I/O,
and OS/FFI. Kani would have to model large foreign data structures, and a proof
would say little about the real terminal. Use the Cucumber/Gherkin scenarios,
unit tests, and screenshot tests for these layers instead. Likewise, BLE and
serial correctness is dominated by CoreBluetooth/termios behavior; hardware
contract tests and explicit teardown are the right tools.

## Limitations

- Kani harnesses use fixed input lengths; they do not prove the unbounded
  `MAX_FRAME_SIZE` path.
- `f32` geometry and trigonometric layout cannot be faithfully model-checked by
  Kani; property tests are more practical there.
- The RPC/session state machines are not yet specified in TLA+/Stateright.
- No proof is a substitute for the hardware BLE/USB smoke tests.
