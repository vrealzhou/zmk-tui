#!/usr/bin/env bash
# Focused mutation run for the pure protocol/label logic.
# The full TUI/hardware layers are intentionally excluded (see .cargo/mutants.toml).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo mutants \
  -f src/protocol/frame.rs \
  -f src/labels.rs \
  -f src/tui/mouse.rs \
  --jobs "${JOBS:-4}" \
  --timeout 60 \
  -- --lib
