# Third-party notices

`zmk-tui` is MIT licensed. It vendors or generates from the following
MIT-licensed ZMK project files:

## ZMK Studio protocol definitions

- Files: `proto/zmk/*.proto`
- Source: <https://github.com/zmkfirmware/zmk-studio-messages>
- License: MIT, Copyright (c) 2024 The ZMK Contributors
- A copy of the license is in `tools/upstream/LICENSE-zmk-studio-messages`.

Generated Rust bindings are produced at build time by `prost-build`; they are
not committed.

## ZMK keycode definitions

- Files: `tools/upstream/keys.h`, `tools/upstream/hid_usage.h`,
  `tools/upstream/hid_usage_pages.h`, `tools/upstream/modifiers.h`
- Source: <https://github.com/zmkfirmware/zmk/tree/main/app/include/dt-bindings/zmk>
- License: MIT, Copyright (c) 2020 The ZMK Contributors
- Based on the USB HID Usage Tables 1.21, Copyright (c) 1996-2020 USB
  Implementers Forum.

`src/keycodes/data.rs` is generated from these headers by
`tools/gen_keycodes.py`. The compact labels in that table are original to this
project.

## Rust dependencies

See `Cargo.lock` for the full dependency tree. Runtime dependencies are
distributed under their respective licenses (MIT / Apache-2.0 / BSD-2-Clause),
including `ratatui`, `crossterm`, `prost`, `serialport`, and `bluest`.
