#!/usr/bin/env python3
"""Gate a cargo-mutants result directory.

Usage:
    python3 tools/check_mutants.py target/quality/mutants/mutants.out \
        --min-score 90
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def read_lines(path: Path) -> list[str]:
    if not path.exists():
        return []
    return [line for line in path.read_text().splitlines() if line.strip()]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("outcomes", type=Path, help="cargo-mutants mutants.out directory")
    parser.add_argument("--min-score", type=float, default=90.0)
    args = parser.parse_args()

    caught = read_lines(args.outcomes / "caught.txt")
    missed = read_lines(args.outcomes / "missed.txt")
    timeout = read_lines(args.outcomes / "timeout.txt")
    unviable = read_lines(args.outcomes / "unviable.txt")
    killed = len(caught)
    total = killed + len(missed) + len(timeout)
    if total == 0:
        sys.exit(f"no mutation outcomes found in {args.outcomes}")

    score = killed / total * 100.0
    print("Mutation gate")
    print(f"  caught:   {killed}")
    print(f"  missed:   {len(missed)}")
    print(f"  timeout:  {len(timeout)}")
    print(f"  unviable: {len(unviable)} (excluded)")
    print(f"  score:    {score:.2f}% (minimum {args.min_score:.2f}%)")
    if score < args.min_score:
        print("  result:   FAIL", file=sys.stderr)
        for line in missed[:10]:
            print(f"  MISSED {line}", file=sys.stderr)
        sys.exit(1)
    print("  result:   PASS")


if __name__ == "__main__":
    main()
