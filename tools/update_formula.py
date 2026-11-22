#!/usr/bin/env python3
"""Point the Homebrew formula at a release's universal macOS binary.

Usage: tools/update_formula.py v0.1.0 <sha256> [owner/repo]
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

FORMULA = Path(__file__).resolve().parent.parent / "Formula" / "zmk-tui.rb"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("tag", help="release tag, e.g. v0.1.0")
    parser.add_argument("sha256", help="sha256 of the release tarball")
    parser.add_argument(
        "repository",
        nargs="?",
        default="vrealzhou/zmk-tui",
        help="GitHub owner/repo (default: vrealzhou/zmk-tui)",
    )
    args = parser.parse_args()

    tag = args.tag.removeprefix("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", tag):
        sys.exit(f"unexpected release tag: {args.tag!r}")
    if not re.fullmatch(r"[0-9a-f]{64}", args.sha256):
        sys.exit("sha256 must be 64 lowercase hex characters")

    url = (
        f"https://github.com/{args.repository}/releases/download/"
        f"v{tag}/zmk-tui-{tag}-universal-apple-darwin.tar.gz"
    )
    text = FORMULA.read_text(encoding="utf-8")
    text, url_count = re.subn(r'^  url ".*"$', f'  url "{url}"', text, count=1, flags=re.M)
    text, sha_count = re.subn(
        r'^  sha256 ".*"$', f'  sha256 "{args.sha256}"', text, count=1, flags=re.M
    )
    text, version_count = re.subn(
        r'^  version ".*"$', f'  version "{tag}"', text, count=1, flags=re.M
    )
    if (url_count, sha_count, version_count) != (1, 1, 1):
        sys.exit("could not find url/sha256/version in the formula")
    FORMULA.write_text(text, encoding="utf-8")
    print(f"updated {FORMULA} for v{tag}")


if __name__ == "__main__":
    main()
