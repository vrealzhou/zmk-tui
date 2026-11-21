#!/usr/bin/env python3
"""Generate src/keycodes/data.rs from ZMK's keycode headers.

Inputs (all MIT licensed, vendored in tools/upstream):
  - keys.h           ZMK keycode macros
  - hid_usage.h      HID usage IDs
  - hid_usage_pages.h HID usage page constants

Only keyboard/keypad (page 7) and consumer (page 12) usages are emitted,
matching the set ZMK Studio exposes for key-press bindings.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UPSTREAM = ROOT / "tools" / "upstream"
OUT = ROOT / "src" / "keycodes" / "data.rs"

# Modifier values used by the keycode encoding (high byte of a keycode).
MODS = {
    "LCTL": 0x01,
    "LSFT": 0x02,
    "LALT": 0x04,
    "LGUI": 0x08,
    "RCTL": 0x10,
    "RSFT": 0x20,
    "RALT": 0x40,
    "RGUI": 0x80,
}

# The short macro names used in expressions such as `LS(A)`.
MOD_ALIASES = {
    "LC": MODS["LCTL"],
    "LS": MODS["LSFT"],
    "LA": MODS["LALT"],
    "LG": MODS["LGUI"],
    "RC": MODS["RCTL"],
    "RS": MODS["RSFT"],
    "RA": MODS["RALT"],
    "RG": MODS["RGUI"],
}

CATEGORIES = [
    "Letters",
    "Numbers",
    "Punctuation",
    "Editing",
    "Navigation",
    "Modifiers",
    "Function",
    "Keypad",
    "Media",
    "System",
    "Other",
]


def joined(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    return re.sub(r"\\\n", " ", text)


def parse_constants(text: str) -> dict[str, int]:
    values: dict[str, int] = {}
    for line in text.splitlines():
        m = re.match(r"\s*#define\s+([A-Za-z_]\w*)\s+\(?(0x[0-9A-Fa-f]+|\d+)\)?\s*(?://.*)?$", line)
        if m:
            values[m.group(1)] = int(m.group(2), 0)
    return values


def balanced(expr: str) -> bool:
    depth = 0
    for ch in expr:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth < 0:
                return False
    return depth == 0


def strip_outer(expr: str) -> str:
    expr = expr.strip()
    while expr.startswith("(") and expr.endswith(")") and balanced(expr[1:-1]):
        expr = expr[1:-1].strip()
    return expr


def split_args(expr: str) -> list[str]:
    args, depth, start = [], 0, 0
    for i, ch in enumerate(expr):
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif ch == "," and depth == 0:
            args.append(expr[start:i])
            start = i + 1
    args.append(expr[start:])
    return [arg.strip() for arg in args]


class Evaluator:
    def __init__(self, constants: dict[str, int], defines: dict[str, str]):
        self.constants = constants
        self.defines = defines
        self.memo: dict[str, int] = {}
        self.busy: set[str] = set()

    def atom(self, token: str) -> int:
        token = token.strip()
        if re.fullmatch(r"0x[0-9A-Fa-f]+|\d+", token):
            return int(token, 0)
        if token in self.constants:
            return self.constants[token]
        return self.eval(token)

    def eval(self, expr: str, depth: int = 0) -> int:
        if depth > 40:
            raise ValueError("macro recursion too deep")
        expr = strip_outer(expr)

        m = re.fullmatch(r"ZMK_HID_USAGE\((.*)\)", expr)
        if m:
            page, usage = (self.atom(a) for a in split_args(m.group(1)))
            return (page << 16) | (usage & 0xFFFF)

        m = re.fullmatch(r"(LS|LC|LA|LG|RS|RC|RA|RG)\((.*)\)", expr)
        if m:
            inner = self.eval(m.group(2), depth + 1)
            return (MOD_ALIASES[m.group(1)] << 24) | inner

        # A handful of media keys use a raw HID usage with a modifier.
        m = re.fullmatch(r"APPLY_MODS\(([^,]+),(.+)\)", expr)
        if m:
            mods = self.eval(m.group(1), depth + 1)
            inner = self.eval(m.group(2), depth + 1)
            return (mods << 24) | inner

        if re.fullmatch(r"[A-Za-z_]\w*", expr):
            if expr in self.memo:
                return self.memo[expr]
            if expr not in self.defines:
                raise KeyError(expr)
            if expr in self.busy:
                raise ValueError(f"recursive define {expr}")
            self.busy.add(expr)
            value = self.eval(self.defines[expr], depth + 1)
            self.busy.discard(expr)
            self.memo[expr] = value
            return value

        m = re.fullmatch(r"(.+?)\s*\|\s*(.+)", expr)
        if m:
            return self.eval(m.group(1), depth + 1) | self.eval(m.group(2), depth + 1)
        m = re.fullmatch(r"(.+?)\s*<<\s*(\d+)", expr)
        if m:
            return self.eval(m.group(1), depth + 1) << int(m.group(2))

        raise ValueError(f"unhandled expression: {expr!r}")


def main() -> None:
    constants = {}
    constants.update(parse_constants(joined(UPSTREAM / "hid_usage_pages.h")))
    constants.update(parse_constants(joined(UPSTREAM / "hid_usage.h")))

    defines: dict[str, str] = {}
    deprecated: set[str] = set()
    canonical_order: list[str] = []
    for raw_line in joined(UPSTREAM / "keys.h").splitlines():
        line = raw_line.strip()
        m = re.match(r"#define\s+([A-Za-z_]\w*)\s+(.+)$", line)
        if not m:
            continue
        name, expr = m.group(1), m.group(2)
        if expr.lstrip().startswith("(") is False and "(" not in expr:
            continue
        expr = re.sub(r"//.*", "", expr).strip()
        if "WARNING: DEPRECATED" in raw_line:
            deprecated.add(name)
        defines[name] = expr
        canonical_order.append(name)

    evaluator = Evaluator(constants, defines)

    # Group all names by encoded value; keep the first non-deprecated name as
    # canonical and collect the rest for search.
    by_value: dict[int, list[str]] = {}
    for name in canonical_order:
        try:
            value = evaluator.eval(name)
        except (KeyError, ValueError):
            continue
        page = (value >> 16) & 0xFF
        if page not in (7, 12) or value == 0:
            continue
        by_value.setdefault(value, []).append(name)

    EDITING = {40, 41, 42, 43, 44, 57, 73, 76}
    NAVIGATION = {74, 75, 77, 78, 79, 80, 81, 82, 101}
    FUNCTION = set(range(58, 70)) | set(range(104, 116))
    KEYPAD = set(range(83, 104)) | {133, 134, 176, 177}
    PUNCTUATION = {45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 100}
    MODIFIERS = set(range(224, 232))
    SYSTEM = {70, 71, 72}
    MEDIA_KEYBOARD = set(range(0xE8, 0xFC))

    def category(page: int, usage: int) -> str:
        if page == 12:
            if usage in {0x30, 0x31, 0x32, 0x33, 0x34, 0x35}:
                return "System"
            return "Media"
        if 4 <= usage <= 29:
            return "Letters"
        if 30 <= usage <= 39:
            return "Numbers"
        if usage in MODIFIERS:
            return "Modifiers"
        if usage in EDITING:
            return "Editing"
        if usage in NAVIGATION:
            return "Navigation"
        if usage in FUNCTION:
            return "Function"
        if usage in KEYPAD:
            return "Keypad"
        if usage in PUNCTUATION:
            return "Punctuation"
        if usage in SYSTEM or usage == 102:
            return "System"
        if usage in MEDIA_KEYBOARD:
            return "Media"
        return "Other"

    KEY_LABELS = {
        40: "ENT", 41: "ESC", 42: "BSPC", 43: "TAB", 44: "SPC",
        45: "-", 46: "=", 47: "[", 48: "]", 49: "\\", 50: "NUHS",
        51: ";", 52: "'", 53: "`", 54: ",", 55: ".", 56: "/",
        57: "CAPS", 70: "PRTSC", 71: "SCRLK", 72: "PAUSE",
        73: "INS", 74: "HOME", 75: "PGUP", 76: "DEL", 77: "END",
        78: "PGDN", 79: "→", 80: "←", 81: "↓", 82: "↑",
        83: "NUMLK", 84: "KP/", 85: "KP*", 86: "KP-", 87: "KP+",
        88: "KPENT", 89: "KP1", 90: "KP2", 91: "KP3", 92: "KP4",
        93: "KP5", 94: "KP6", 95: "KP7", 96: "KP8", 97: "KP9",
        98: "KP0", 99: "KP.", 100: "NUBS", 101: "MENU", 102: "PWR",
        103: "KP=", 133: "KP,", 134: "KP=", 176: "KP00", 177: "KP000",
        224: "LCTL", 225: "LSFT", 226: "LALT", 227: "LGUI",
        228: "RCTL", 229: "RSFT", 230: "RALT", 231: "RGUI",
    }
    for usage in range(0xE8, 0xFC):
        KEY_LABELS[usage] = {
            0xE8: "PLAY", 0xE9: "STOP", 0xEA: "PREV", 0xEB: "NEXT",
            0xEC: "EJECT", 0xED: "VOL+", 0xEE: "VOL-", 0xEF: "MUTE",
            0xF0: "WWW", 0xF1: "BACK", 0xF2: "FWD", 0xF3: "STOP",
            0xF4: "FIND", 0xF5: "SCR+", 0xF6: "SCR-", 0xF7: "EDIT",
            0xF8: "SLEEP", 0xF9: "LOCK", 0xFA: "REFR", 0xFB: "CALC",
        }[usage]

    SHIFTED_SYMBOLS = {
        30: "!", 31: "@", 32: "#", 33: "$", 34: "%", 35: "^", 36: "&",
        37: "*", 38: "(", 39: ")", 45: "_", 46: "+", 47: "{", 48: "}",
        49: "|", 51: ":", 52: '"', 53: "~", 54: "<", 55: ">", 56: "?",
        100: "|",
    }

    CONSUMER_LABELS = {
        "C_VOLUME_UP": "VOL+", "C_VOLUME_DOWN": "VOL-", "C_MUTE": "MUTE",
        "C_PLAY_PAUSE": "PLAY", "C_PLAY": "PLAY", "C_PAUSE": "PAUS",
        "C_NEXT": "NEXT", "C_PREVIOUS": "PREV", "C_STOP": "STOP",
        "C_EJECT": "EJECT", "C_RECORD": "REC", "C_FAST_FORWARD": "FFWD",
        "C_REWIND": "REW", "C_BRIGHTNESS_INC": "BRI+",
        "C_BRIGHTNESS_DEC": "BRI-", "C_AL_CALCULATOR": "CALC",
        "C_AL_EMAIL": "MAIL", "C_AL_WWW": "WWW", "C_AL_MY_COMPUTER": "MYPC",
        "C_AL_LOCK": "LOCK", "C_AC_SEARCH": "SRCH", "C_AC_HOME": "HOME",
        "C_AC_BACK": "BACK", "C_AC_FORWARD": "FWD", "C_AC_STOP": "STOP",
        "C_AC_REFRESH": "REFR", "C_AC_COPY": "COPY", "C_AC_PASTE": "PASTE",
        "C_AC_CUT": "CUT", "C_AC_UNDO": "UNDO", "C_AC_REDO": "REDO",
        "C_AC_NEW": "NEW", "C_AC_SAVE": "SAVE", "C_AC_CLOSE": "CLOSE",
        "C_AC_PRINT": "PRINT", "C_POWER": "PWR", "C_SLEEP": "SLEEP",
        "C_RESET": "RESET", "C_AC_NEXT_KEYBOARD_LAYOUT_SELECT": "GLOBE",
    }

    def base_label(page: int, usage: int, canonical: str) -> str:
        if page == 7:
            if 4 <= usage <= 29:
                return chr(ord("A") + usage - 4)
            if 30 <= usage <= 38:
                return str(usage - 29)
            if usage == 39:
                return "0"
            if 58 <= usage <= 69:
                return f"F{usage - 57}"
            if 104 <= usage <= 115:
                return f"F{usage - 91}"
            if usage in KEY_LABELS:
                return KEY_LABELS[usage]
            return canonical[:5]
        if canonical in CONSUMER_LABELS:
            return CONSUMER_LABELS[canonical]
        if canonical.startswith("C_"):
            return canonical[2:][:5]
        return canonical[:5]

    def label(page: int, usage: int, mods: int, canonical: str) -> str:
        base = base_label(page, usage, canonical)
        if mods == 0:
            return base
        if mods == MODS["LSFT"] and usage in SHIFTED_SYMBOLS:
            return SHIFTED_SYMBOLS[usage]
        prefix = ""
        if mods & MODS["LCTL"] or mods & MODS["RCTL"]:
            prefix += "⌃"
        if mods & MODS["LALT"] or mods & MODS["RALT"]:
            prefix += "⌥"
        if mods & MODS["LSFT"] or mods & MODS["RSFT"]:
            prefix += "⇧"
        if mods & MODS["LGUI"] or mods & MODS["RGUI"]:
            prefix += "⌘"
        return prefix + base

    entries = []
    for value, names in by_value.items():
        preferred = [name for name in names if name not in deprecated]
        canonical = (preferred or names)[0]
        aliases = sorted({name for name in names if name != canonical and name not in deprecated})
        page = (value >> 16) & 0xFF
        usage = value & 0xFFFF
        mods = (value >> 24) & 0xFF
        entries.append({
            "value": value,
            "page": page,
            "usage": usage,
            "mods": mods,
            "name": canonical,
            "aliases": aliases,
            "label": label(page, usage, mods, canonical),
            "category": category(page, usage),
        })

    entries.sort(key=lambda e: (CATEGORIES.index(e["category"]), e["page"], e["usage"], e["mods"]))

    def rust_string(text: str) -> str:
        return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'

    lines = [
        "// @generated by tools/gen_keycodes.py — do not edit by hand.",
        "//",
        "// Source: ZMK's dt-bindings/zmk/keys.h, hid_usage.h and",
        "// hid_usage_pages.h (MIT, Copyright (c) 2020 The ZMK Contributors),",
        "// themselves based on the USB HID Usage Tables 1.21.",
        "",
        "use super::{Category, Keycode};",
        "",
        "pub static KEYCODES: &[Keycode] = &[",
    ]
    for entry in entries:
        aliases = ", ".join(rust_string(a) for a in entry["aliases"])
        lines.append(
            "    Keycode { "
            f"value: 0x{entry['value']:08X}, page: {entry['page']}, id: {entry['usage']}, "
            f"mods: 0x{entry['mods']:02X}, name: {rust_string(entry['name'])}, "
            f"aliases: &[{aliases}], label: {rust_string(entry['label'])}, "
            f"category: Category::{entry['category']} }},"
        )
    lines.append("];")
    lines.append("")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote {OUT} with {len(entries)} keycodes")


if __name__ == "__main__":
    main()
