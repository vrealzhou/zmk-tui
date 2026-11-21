# Developer guide

This guide covers the implementation, protocol, build, testing and maintenance
details. For installing and using the editor, see the [README](../README.md).

## Architecture

```text
TUI (ratatui/crossterm)
  └─ StudioClient: request IDs, timeout, response matching, lock/unsaved events
      └─ framed protobuf messages: 0xAB payload 0xAD
          ├─ USB CDC-ACM: serialport, reader/writer threads
          └─ BLE GATT: bluest, write characteristic + indications
              └─ ZMK Studio RPC in the keyboard firmware
```

| Path | Responsibility |
| --- | --- |
| `src/client.rs` | ZMK Studio RPC client, request correlation, typed errors, notifications |
| `src/protocol/frame.rs` | Incremental framing/decoder with escaping and resynchronisation |
| `src/protocol/messages.rs` | `prost`-generated protobuf types |
| `src/transport/serial.rs` | USB CDC-ACM transport and device enumeration |
| `src/transport/ble.rs` | BLE scan/connect/GATT transport and teardown |
| `src/transport/mod.rs` | `Connection`, `Sink`, `FrameResult` abstractions |
| `src/model.rs` | Keymap, layout, behavior and device-info domain types |
| `src/layout.rs` | Physical-layout geometry, grid placement and hit-testing |
| `src/labels.rs` | Binding/detail labels for the keymap view |
| `src/keycodes/` | HID keycode table, parsing, search and modifier symbols |
| `src/tui/` | App state machine, rendering, modals, keyboard grid, mouse hit map |
| `src/simulator.rs` | In-process ZMK Studio device used by tests and `--simulate` |
| `proto/zmk/` | Vendored ZMK Studio message schemas |
| `tests/` | Client integration tests, TUI tests, Gherkin features and step definitions |
| `tools/` | CRAP, mutation, screenshot and keycode-generation utilities |

## Protocol

ZMK Studio is an application-level RPC protocol defined by ZMK; it is not a
generic standard like HID. The underlying transports are standard USB CDC-ACM
and Bluetooth GATT.

### Framing

`src/protocol/frame.rs` implements the byte stream:

```text
0xAB  start of frame
0xAC  escape
0xAD  end of frame
```

Reserved bytes in the payload are prefixed with `0xAC`, including `0xAC`
itself. Frames larger than `MAX_FRAME_SIZE` (256 KiB) are rejected. The decoder
is incremental and ignores noise outside a frame; a new `0xAB` restarts a
partial frame.

### Messages

The payload is a protobuf `zmk.studio.Request` or `zmk.studio.Response`. The
schemas are vendored from
[zmk-studio-messages](https://github.com/zmkfirmware/zmk-studio-messages):

- `studio.proto`: request/response envelope and error metadata;
- `core.proto`: device info, lock state and reset;
- `keymap.proto`: keymap, layers, physical layouts, save/discard;
- `behaviors.proto`: behavior metadata and parameter descriptions;
- `meta.proto`: RPC metadata.

`build.rs` compiles them with `prost-build`; `protoc-bin-vendored` supplies
`protoc`, so a system protobuf installation is not required.

### Request correlation

The client serialises mutations and allows one request in flight. Each request
has a monotonically increasing `request_id`; the reader task matches
`RequestResponse` messages to a `oneshot` sender. Read requests retry once on
timeout; mutation requests are never retried automatically. Lock and unsaved
state notifications are forwarded through a broadcast channel. A missing or
unsupported request surfaces as `RpcNotFound`.

The protocol currently corresponds to ZMK Studio messages v3. There is no
firmware version handshake.

## USB transport

The firmware must be built with Studio plus the USB UART transport:

```yaml
snippet: studio-rpc-usb-uart
cmake-args: -DCONFIG_ZMK_STUDIO=y
```

The keyboard then exposes a CDC-ACM serial interface (`/dev/cu.usbmodem…` on
macOS, `/dev/ttyACM…` on Linux, `COMx` on Windows). HID and Studio are separate
interfaces on the same USB device; the editor only uses the CDC-ACM channel.

`src/transport/serial.rs`:

- enumerates ports with `serialport::available_ports()` and keeps USB ports;
- opens the port exclusively at 9600 8N1 with a 100 ms read timeout (the baud
  rate is mostly nominal for CDC-ACM);
- runs a dedicated reader thread with a frame decoder and a writer thread;
- sends frames through a Tokio channel to the RPC client.

On macOS the app prefers `/dev/cu.*` over the `/dev/tty.*` twin. A port can only
have one exclusive owner, which is why `lsof /dev/cu.usbmodem…` is useful when
another Studio client is open.

## BLE transport

ZMK Studio uses this GATT service:

```text
Service:        00000000-0196-6107-c967-c5cfb1c2482a
Characteristic: 00000001-0196-6107-c967-c5cfb1c2482a
Properties:     Write, Read, Indicate
```

`src/transport/ble.rs` uses `bluest`:

1. `Adapter::default()` and `discover_devices(&[STUDIO_SERVICE_UUID])`;
2. read and store the `DeviceId` while the scan adapter is alive;
3. open a fresh adapter and `open_device(&id)`, then `connect_device`;
4. discover the Studio service and RPC characteristic;
5. subscribe to **indications** with `notify()` (CCC = `0x0002`);
6. write requests in chunks bounded by `max_write_len_async()` (20–180 bytes,
   clamped) and receive response chunks as indications;
7. feed each indication chunk through the same incremental frame decoder;
8. on drop, abort the notify task and asynchronously disconnect the device.

ZMK's advertising normally contains HID and battery services, not the Studio
UUID, so discovery depends on the keyboard already being connected to this
computer. Pairing/encryption is handled by the OS Bluetooth stack; the
characteristic requires an encrypted link. The app does not implement pairing.

CoreBluetooth-specific robustness notes:

- Read `DeviceId` while the scan adapter is still alive; messaging a peripheral
  after its adapter has been dropped can panic in objc2.
- Re-open the peripheral by ID on a fresh adapter; reusing the scan adapter can
  leave writes working while indications are never delivered.
- `Cargo.toml` disables debug assertions for `objc2`/`objc2-foundation` in the
  dev profile because objc2 0.6.5 mis-verifies
  `-[__NSConcreteUUID getUUIDBytes:]`. Release builds do not run that check.

## Connection and snapshot flow

After the transport is established, the client queries:

1. `core.get_device_info`;
2. `core.get_lock_state`;
3. if unlocked: `keymap.get_keymap`, behavior details for each behavior ID, and
   `keymap.get_physical_layouts` / active layout;
4. if locked: the unlock dialog, with automatic re-polling and notification
   handling.

Binding edits go through `keymap.set_layer_binding`; changes are live but only
persist after `keymap.save_changes`. Layer and layout operations use the
corresponding `keymap.*` requests. `discard_changes` restores the last saved
settings, not the compiled keymap.

## Layout detection

Layout selection comes from the firmware, not from a fixed board list:

- `keymap.get_keymap` supplies layers and bindings;
- `keymap.get_physical_layouts` supplies key geometry and display names;
- the active layout's geometry drives drawing and directional navigation;
- `p` lists/switches layouts via `set_active_physical_layout`;
- firmware without the layout RPC, or with zero layouts, falls back to a
  generic 10-column grid while remaining editable.

`CONFIG_ZMK_KEYBOARD_NAME` is shown as the device name. Studio does not expose a
board model; geometry and layout names are authoritative.

## Layers, slots and storage

The TUI shows every layer the firmware reports and lets the layer manager use
the free slots advertised by `available_layers`. Studio cannot create more
layers than the firmware was built for. A firmware keymap can reserve future
layers for Studio:

```dts
keymap {
    compatible = "zmk,keymap";

    base { display-name = "Base"; bindings = /* ... */; };
    /* ... existing layers ... */

    extra1 {
        status = "reserved";
    };
};
```

Reserved layers are ignored by normal builds but can be activated through
Studio. The built-in simulators expose Base, Lower, Raise, Adjust and Media.

Saved Studio settings override the compiled `.keymap` on subsequent boots. `R`
restores the firmware's stock settings; discarding only reverts to the last
saved settings.

## Libraries and upstream projects

| Crate/library | Role |
| --- | --- |
| `serialport` | USB CDC-ACM enumeration and blocking I/O |
| `bluest` | Cross-platform BLE; CoreBluetooth on macOS, BlueZ on Linux, WinRT on Windows |
| `prost` / `prost-build` | Protobuf runtime and code generation |
| `protoc-bin-vendored` | Bundled protoc for builds |
| `tokio` | Async runtime, channels, timeouts and spawned tasks |
| `futures` | BLE indication stream |
| `ratatui` / `crossterm` | Terminal UI, input and mouse capture |

ZMK and ZMK Studio are MIT-licensed projects by The ZMK Contributors. The
protocol schemas and generated keycode data are vendored in this repository;
the app is not affiliated with or endorsed by the ZMK project. There is no
official Rust SDK for ZMK Studio, so the client and transports are implemented
here against the vendored schemas.

## Build and generated code

```bash
cargo build --release
```

`build.rs` compiles `proto/zmk/*.proto` into `src/protocol/messages.rs` at build
time. `tools/gen_keycodes.py` regenerates `src/keycodes/data.rs` from the
vendored ZMK headers; see the script header for usage. `THIRD_PARTY.md` records
upstream licenses.

## Testing

```bash
cargo test --all-targets
cargo test --doc
cargo test --test cucumber
cargo clippy --all-targets
cargo fmt --check
```

The suite contains 99 Rust unit/integration tests plus 4 Gherkin scenarios
(31 steps). Test targets:

- library unit tests: framing, keycodes, labels, layout, modal state, mouse
  hit-testing, serial error handling;
- `tests/client.rs`: in-process RPC client and simulator integration;
- `tests/tui.rs`: complete `TestBackend` UI flows;
- `tests/cucumber.rs`: Gherkin step definitions driving the real App;
- `tests/features/`: binding edit/save, unsaved-change quit, layers/boards.

The simulator implements the same wire protocol, lock state, notifications,
save/discard and layer operations as the firmware.

### Quality gate

The local/CI gate is [`../tools/quality-gate.sh`](../tools/quality-gate.sh).
Prerequisites: `cargo-llvm-cov`, `cargo-mutants`, Kani and
`python3 -m pip install lizard`.

```bash
bash tools/quality-gate.sh
# Skip only the slow mutation stage while iterating:
SKIP_MUTATION=1 bash tools/quality-gate.sh
```

It runs, in order:

1. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`;
2. unit tests (`--lib`, `--bins`);
3. integration tests (`tests/client.rs`, `tests/tui.rs`, `tests/cucumber.rs`);
4. coverage + CRAP budgets:
   - exclude `src/transport/**`, `src/main.rs`, `src/tui/mod.rs`,
     `src/verification.rs` (Kani-only code is not part of runtime coverage);
   - CRAP > 30: **0**;
   - CRAP > 100: **0**;
   - line coverage: **≥ 79%**;
5. formal verification: Kani codegen for all harnesses plus the fast
   `encode_escapes_each_reserved_single_byte` proof;
6. mutation testing: frame codec, mutation score **≥ 90%** (currently 19/20,
   the survivor being an equivalent allocation-size mutant).

CI runs the same gate in
[`../.github/workflows/quality-gate.yml`](../.github/workflows/quality-gate.yml)
on macOS, installing `cargo-llvm-cov`, `cargo-mutants` and Kani. Artifacts
(`target/quality/coverage.json`, `crap-report.md`, mutation outcomes) are
uploaded even when the gate fails.

### CRAP and coverage

`tools/crap.py` combines `cargo llvm-cov --json` with lizard complexity:

```bash
cargo llvm-cov --all-targets --json --output-path /tmp/coverage.json
PYTHONPATH=/tmp/zmk-py python3 tools/crap.py /tmp/coverage.json \
  --exclude 'src/verification.rs' --top 30 --output docs/crap-report.md
```

See [crap-baseline.md](crap-baseline.md), [crap-report.md](crap-report.md) and
[crap-summary.md](crap-summary.md). The current high-CRAP remainder is mostly
FFI/OS/entry code and large dispatch functions; it is documented rather than
hidden by coverage exclusions.

#### Why some functions show 0% coverage

`cargo llvm-cov` only records code executed by the test process. The remaining
runtime zeros are OS/hardware/entry code:

- BLE/Serial: `connect`, `scan`, `adapter`, `send`, `spawn_reader`;
- terminal/entry: `run`, `run_loop`, `spawn_input_thread`;
- CLI discovery: `list_devices`;
- hardware-spawning selectors: `connect_choice`, `connect_by_bluetooth_name`.

Tests deliberately use `Connection::duplex` and `TestBackend`, so no hardware
is touched. These files are excluded from the CRAP gate. Kani-only code
(`src/verification.rs`) and `#[cfg(test)]` modules are also filtered from the
runtime report, so they cannot masquerade as uncovered production code. The
report labels each remaining 0% row either as **Glue** (hardware/terminal/entry
point) or as an **unclassified test gap** (low-CRAP accessors and rendering
edge cases); the latter are normal candidates for future unit tests.

### Mutation testing

`cargo-mutants` is configured in `../.cargo/mutants.toml`; the focused runner is
`../tools/mutate.sh`. The frame-codec suite kills 19/20 mutants (the survivor is
a `Vec::with_capacity` arithmetic equivalent). Keycode results are partial; see
[mutation-report.md](mutation-report.md) for exact numbers and reproduction
commands.

### Formal verification

The frame codec is the strongest bounded-verification target. Kani harnesses
live in `../src/verification.rs`:

```bash
cargo kani --harness frame_round_trips_arbitrary_payload
cargo kani --harness arbitrary_serial_noise_never_panics
cargo kani --harness encoded_frame_shape_is_well_formed
```

The harness crate passes `cargo kani --only-codegen`, and the fast
`encode_escapes_each_reserved_single_byte` proof passes in the quality gate
(1.4s, 418 checks). Full round-trip/noise CBMC runs exceed the local time budget;
they remain proof obligations for a dedicated verification job. See
[formal-verification.md](formal-verification.md) for the Kani/Prusti/Creusot/
TLA+ candidate map and limitations.

### Screenshots

```bash
cargo run --example screens -- /tmp/zmk-shots
python3 tools/render_screenshot.py /tmp/zmk-shots docs
```

## Developer troubleshooting

### BLE session stuck / timeout

A connect-then-timeout with TX but no RX usually means the BLE Studio RPC
session is held by another client or the keyboard's GATT subscription is stale.
Force-release in this order:

1. Quit every `zmk-tui`: `pkill -TERM -x zmk-tui`, then `pkill -KILL -x zmk-tui`
   if necessary.
2. Quit ZMK Studio (native and web) and any browser tab using Web Bluetooth;
   closing the browser tab releases its GATT client.
3. Power-cycle the keyboard (off/on) to clear firmware GATT/RPC state.
4. Toggle macOS Bluetooth off/on (or `blueutil -p 0; sleep 2; blueutil -p 1`).
5. Remove/re-pair the keyboard in System Settings if the cache is still stale.

For transport logging, launch with `ZMK_TUI_BLE_DEBUG=1` and redirect stderr:

```bash
ZMK_TUI_BLE_DEBUG=1 ./target/release/zmk-tui --ble "Eyelash Sofle" 2>/tmp/ble.log
```

Logs include `connect_device`, `subscribed to indications`, `tx N bytes`,
`rx N bytes`, the CCC value, and notification errors. Use USB as a fallback when
the serial port is free.

### USB port busy

The TUI needs exclusive access. Check `lsof /dev/cu.usbmodem…`, close ZMK
Studio/Web Serial/serial monitors/other TUI instances, and retry. On Linux,
ensure the user is in `dialout` or `uucp`.

### One more implementation note

The frame decoder intentionally resynchronises on noise rather than failing;
this makes BLE chunk boundaries and USB read boundaries interchangeable. Do not
add transport-specific framing logic without preserving that property.
