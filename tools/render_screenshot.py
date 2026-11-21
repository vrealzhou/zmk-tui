#!/usr/bin/env python3
"""Render zmk-tui cell dumps (from examples/screens.rs) into PNG screenshots."""

from __future__ import annotations

import json
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

TITLE_H = 52
BORDER = 2
SCALE_FONT = 28
CELL_H = 37
CELL_W = 18
BG = (15, 17, 23)
PANEL = (23, 26, 35)
BORDER_C = (42, 48, 64)
TEXT = (214, 220, 232)
TITLE_TEXT = (122, 162, 247)
MUTED = (125, 134, 155)

MENLO = "/System/Library/Fonts/Menlo.ttc"


def load_font(size: int, bold: bool) -> ImageFont.FreeTypeFont:
    # Menlo.ttc: regular, bold, italic, bold italic
    return ImageFont.truetype(MENLO, size, index=1 if bold else 0)


def blend(fg, bg, amount: float):
    return tuple(round(f * amount + b * (1 - amount)) for f, b in zip(fg, bg))


def color(value, fallback):
    if value is None:
        return fallback
    if isinstance(value, list) and len(value) == 3 and all(isinstance(v, int) for v in value):
        return tuple(value)
    return fallback


def render(data: dict, label: str, out: Path) -> None:
    width = data["width"]
    height = data["height"]
    img_w = width * CELL_W + BORDER * 2
    img_h = TITLE_H + height * CELL_H + BORDER * 2
    img = Image.new("RGB", (img_w, img_h), BG)
    draw = ImageDraw.Draw(img)

    # Thin frame + title bar.
    draw.rectangle([0, 0, img_w - 1, img_h - 1], outline=BORDER_C, width=BORDER)
    draw.rectangle([BORDER, BORDER, img_w - BORDER - 1, TITLE_H], fill=PANEL)
    for i, dot in enumerate([(224, 108, 117), (229, 192, 123), (158, 206, 106)]):
        cx = BORDER + 18 + i * 20
        draw.ellipse([cx - 5, TITLE_H // 2 - 5, cx + 5, TITLE_H // 2 + 5], fill=dot)
    title_font = load_font(22, bold=True)
    small_font = load_font(18, bold=False)
    draw.text((BORDER + 88, TITLE_H // 2), "zmk-tui", font=title_font, fill=TITLE_TEXT, anchor="lm")
    draw.text(
        (img_w - BORDER - 20, TITLE_H // 2),
        label,
        font=small_font,
        fill=MUTED,
        anchor="rm",
    )

    normal = load_font(SCALE_FONT, bold=False)
    bold = load_font(SCALE_FONT, bold=True)
    top = TITLE_H + BORDER

    for y, row in enumerate(data["rows"]):
        for x, cell in enumerate(row):
            bx = BORDER + x * CELL_W
            by = top + y * CELL_H
            bg = color(cell.get("bg"), BG)
            if bg != BG:
                draw.rectangle([bx, by, bx + CELL_W - 1, by + CELL_H - 1], fill=bg)
            ch = cell.get("ch") or ""
            if not ch.strip():
                continue
            fg = color(cell.get("fg"), TEXT)
            if cell.get("dim"):
                fg = blend(fg, bg, 0.58)
            draw.text(
                (bx + CELL_W // 2, by + CELL_H // 2),
                ch,
                font=bold if cell.get("bold") else normal,
                fill=fg,
                anchor="mm",
            )

    out.parent.mkdir(parents=True, exist_ok=True)
    img.save(out)
    print(f"wrote {out} ({img_w}×{img_h})")


def main() -> None:
    source = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/zmk-shots")
    output = Path(sys.argv[2] if len(sys.argv) > 2 else "docs")
    labels = {
        "editor": "Keymap editor · Corne (simulator)",
        "binding-behavior": "Edit binding · choose behavior",
        "binding-filtered": "Edit binding · filtered behaviors",
        "binding-param": "Edit binding · parameter picker",
        "layers": "Layer manager",
        "help": "Help",
        "sofle-editor": "Keymap editor · Sofle (simulator)",
        "sofle-binding-param": "Edit binding · Sofle",
        "sofle-layers": "Layer manager · Sofle",
    }
    for name in [
        "editor",
        "binding-behavior",
        "binding-filtered",
        "binding-param",
        "layers",
        "help",
        "sofle-editor",
        "sofle-binding-param",
        "sofle-layers",
    ]:
        path = source / f"{name}.json"
        if not path.exists():
            continue
        with path.open() as handle:
            render(json.load(handle), labels.get(name, name), output / "screenshots" / f"{name}.png")


if __name__ == "__main__":
    main()
