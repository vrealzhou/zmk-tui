#!/usr/bin/env python3
"""Point the Homebrew formula at a release's prebuilt binaries.

Usage: tools/update_formula.py v0.1.0 <macos-sha256> <linux-sha256> [owner/repo]
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

FORMULA = Path(__file__).resolve().parent.parent / "Formula" / "zmk-tui.rb"


def replace_ordered(text: str, pattern: str, replacements: list[str]) -> str:
    """Replace regex matches in order, preserving the match's prefix/suffix."""
    index = 0

    def repl(match: re.Match[str]) -> str:
        nonlocal index
        if index >= len(replacements):
            return match.group(0)
        value = replacements[index]
        index += 1
        return f"{match.group(1)}{value}{match.group(2)}"

    text = re.sub(pattern, repl, text, flags=re.M)
    if index != len(replacements):
        sys.exit(f"expected {len(replacements)} matches, found {index}")
    return text


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("tag", help="release tag, e.g. v0.1.0")
    parser.add_argument("mac_sha256", help="sha256 of the universal macOS tarball")
    parser.add_argument("linux_sha256", help="sha256 of the Linux x86_64 tarball")
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
    for name, sha in (("macOS", args.mac_sha256), ("Linux", args.linux_sha256)):
        if not re.fullmatch(r"[0-9a-f]{64}", sha):
            sys.exit(f"{name} sha256 must be 64 lowercase hex characters")

    base = f"https://github.com/{args.repository}/releases/download/v{tag}"
    mac_url = f"{base}/zmk-tui-{tag}-universal-apple-darwin.tar.gz"
    linux_url = f"{base}/zmk-tui-{tag}-x86_64-unknown-linux-gnu.tar.gz"

    text = FORMULA.read_text(encoding="utf-8")
    # The formula has two macOS arch blocks (same universal asset) followed by
    # the Linux x86_64 block.
    text = replace_ordered(text, r'^(\s*url ").*(")$', [mac_url, mac_url, linux_url])
    text = replace_ordered(
        text,
        r'^(\s*sha256 ").*(")$',
        [args.mac_sha256, args.mac_sha256, args.linux_sha256],
    )
    text, version_count = re.subn(
        r'^  version ".*"$', f'  version "{tag}"', text, count=1, flags=re.M
    )
    if version_count != 1:
        sys.exit("could not find version in the formula")
    FORMULA.write_text(text, encoding="utf-8")
    print(f"updated {FORMULA} for v{tag}")


if __name__ == "__main__":
    main()
