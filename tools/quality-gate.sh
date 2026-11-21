#!/usr/bin/env bash
# Local/CI quality gate:
#   formatting + clippy, unit tests, integration tests, CRAP/coverage, mutation.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo-llvm-cov >/dev/null; then
  echo "cargo-llvm-cov is required: cargo install cargo-llvm-cov" >&2
  exit 2
fi
if [[ "${SKIP_MUTATION:-0}" != "1" ]] && ! command -v cargo-mutants >/dev/null; then
  echo "cargo-mutants is required: cargo install cargo-mutants" >&2
  exit 2
fi
if ! python3 -c 'import lizard' >/dev/null 2>&1; then
  echo "lizard is required: python3 -m pip install lizard" >&2
  exit 2
fi
if ! command -v cargo-kani >/dev/null; then
  echo "Kani is required: https://model-checking.github.io/kani/" >&2
  exit 2
fi

echo "== formatting =="
cargo fmt --check

echo "== clippy =="
cargo clippy --all-targets -- -D warnings

echo "== unit tests =="
cargo test --lib
cargo test --bins

echo "== integration tests =="
cargo test --test client
cargo test --test tui
cargo test --test cucumber

mkdir -p target/quality
echo "== coverage + CRAP =="
cargo llvm-cov --all-targets --json --output-path target/quality/coverage.json --quiet
python3 tools/crap.py target/quality/coverage.json \
  --exclude 'src/transport/*' \
  --exclude 'src/main.rs' \
  --exclude 'src/tui/mod.rs' \
  --max-over-30 0 \
  --max-over-100 0 \
  --min-lines 79 \
  --top 30 \
  --output target/quality/crap-report.md

echo "== formal verification (bounded Kani proof) =="
cargo kani --only-codegen
cargo kani --harness encode_escapes_each_reserved_single_byte

if [[ "${SKIP_MUTATION:-0}" == "1" ]]; then
  echo "== mutation: skipped (SKIP_MUTATION=1) =="
  exit 0
fi

echo "== mutation (frame codec) =="
rm -rf target/quality/mutants
set +e
cargo mutants \
  -f src/protocol/frame.rs \
  --jobs "${MUTATION_JOBS:-4}" \
  --timeout 60 \
  --output target/quality/mutants \
  -- --lib
mutation_exit=$?
set -e
python3 tools/check_mutants.py target/quality/mutants/mutants.out --min-score 90
if [[ "$mutation_exit" -ne 0 && "$mutation_exit" -ne 2 ]]; then
  echo "cargo-mutants exited unexpectedly: $mutation_exit" >&2
  exit "$mutation_exit"
fi

echo "== quality gate: PASS =="
