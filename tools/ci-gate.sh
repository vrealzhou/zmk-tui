#!/usr/bin/env bash
# GitHub Actions gate. CRAP/coverage and mutation are local-only checks and are
# deliberately disabled here; run tools/quality-gate.sh locally for the full gate.
set -euo pipefail
cd "$(dirname "$0")/.."

SKIP_CRAP=1 SKIP_MUTATION=1 exec bash tools/quality-gate.sh
