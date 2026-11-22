#!/usr/bin/env python3
"""Compose a side-by-side Before / After PNG for Discord release announcements.

Accepts either two PNG images (--before / --after) or two plain/ANSI terminal
frame text files (--before-text / --after-text) captured from the previous and
new jcode binaries, and produces a crisp dark-themed comparison card.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError as error:  # pragma: no cover - Pillow is installed in CI/dev
    raise SystemExit(f"Pillow is required for compose_before_after.py: {error}") from error


BG_COLOR = (15, 17, 23)
CARD_BG = (22, 26, 35)
CARD_BORDER = (45, 52, 68)
TITLE_COLOR = (238, 241, 248)
TEXT_COLOR = (220, 226, 238)
BEFORE_BADGE_BG = (68, 34, 38)
BEFORE_BADGE_FG = (248, 150, 150)
AFTER_BADGE_BG = (26, 62, 46)
AFTER_BADGE_FG = (120, 232, 168)

ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
FONT_CANDIDATES = (
    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf",
    "/usr/share/fonts/TTF/JetBrainsMonoNLNerdFont-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
)


def load_mono_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    for candidate in FONT_CANDIDATES:
        if Path(candidate).is_file():
            try:
                return ImageFont.truetype(candidate, size=size)
            except OSError:
                continue
    return ImageFont.load_default()


def render_text_frame(
    text: str,
    *,
    font_size: int = 14,
    padding: int = 16,
    max_cols: int = 120,
    max_rows: int = 40,
) -> Image.Image:
    clean = ANSI_ESCAPE_RE.sub("", text).replace("\r\n", "\n").replace("\r", "\n")
    raw_lines = [line[:max_cols] for line in clean.splitlines()[:max_rows]] or [""]
    font = load_mono_font(font_size)
    bbox = font.getbbox("M")
    char_w = max(7, bbox[2] - bbox[0])
    line_h = max(font_size + 4, (bbox[3] - bbox[1]) + 6)
    cols = max(24, max(len(line) for line in raw_lines))
    width = cols * char_w + padding * 2
    height = len(raw_lines) * line_h + padding * 2
    img = Image.new("RGB", (width, height), CARD_BG)
    draw = ImageDraw.Draw(img)
    y = padding
    for line in raw_lines:
        draw.text((padding, y), line, fill=TEXT_COLOR, font=font)
        y += line_h
    return img


def compose_before_after(
    before_img: Image.Image,
    after_img: Image.Image,
    *,
    before_label: str = "Before",
    after_label: str = "After",
    title: str | None = None,
) -> Image.Image:
    before = before_img.convert("RGB")
    after = after_img.convert("RGB")

    # Normalize heights if they differ slightly while preserving aspect ratio.
    target_h = max(before.height, after.height)
    if before.height != target_h:
        scaled_w = max(1, round(before.width * (target_h / before.height)))
        before = before.resize((scaled_w, target_h), Image.Resampling.LANCZOS)
    if after.height != target_h:
        scaled_w = max(1, round(after.width * (target_h / after.height)))
        after = after.resize((scaled_w, target_h), Image.Resampling.LANCZOS)

    outer_pad = 24
    gap = 20
    card_pad = 10
    badge_h = 34
    title_h = 38 if title else 0

    card_w_before = before.width + card_pad * 2
    card_w_after = after.width + card_pad * 2
    card_h = badge_h + target_h + card_pad * 2

    total_w = outer_pad * 2 + card_w_before + gap + card_w_after
    total_h = outer_pad * 2 + title_h + card_h

    canvas = Image.new("RGB", (total_w, total_h), BG_COLOR)
    draw = ImageDraw.Draw(canvas)
    title_font = load_mono_font(18)
    badge_font = load_mono_font(14)

    top_y = outer_pad
    if title:
        draw.text((outer_pad, top_y + 4), title, fill=TITLE_COLOR, font=title_font)
        top_y += title_h

    def draw_panel(
        x: int,
        panel_w: int,
        img: Image.Image,
        label: str,
        badge_bg: tuple[int, int, int],
        badge_fg: tuple[int, int, int],
    ) -> None:
        draw.rounded_rectangle(
            [x, top_y, x + panel_w - 1, top_y + card_h - 1],
            radius=10,
            fill=CARD_BG,
            outline=CARD_BORDER,
            width=1,
        )
        pill_x0 = x + card_pad
        pill_y0 = top_y + 7
        text_box = badge_font.getbbox(label)
        pill_w = (text_box[2] - text_box[0]) + 18
        pill_h = 22
        draw.rounded_rectangle(
            [pill_x0, pill_y0, pill_x0 + pill_w, pill_y0 + pill_h],
            radius=6,
            fill=badge_bg,
        )
        draw.text((pill_x0 + 9, pill_y0 + 3), label, fill=badge_fg, font=badge_font)
        canvas.paste(img, (x + card_pad, top_y + badge_h + card_pad))

    draw_panel(
        outer_pad,
        card_w_before,
        before,
        before_label,
        BEFORE_BADGE_BG,
        BEFORE_BADGE_FG,
    )
    draw_panel(
        outer_pad + card_w_before + gap,
        card_w_after,
        after,
        after_label,
        AFTER_BADGE_BG,
        AFTER_BADGE_FG,
    )
    return canvas


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, help="Path to before PNG image")
    parser.add_argument("--after", type=Path, help="Path to after PNG image")
    parser.add_argument("--before-text", type=Path, help="Path to before TUI frame text")
    parser.add_argument("--after-text", type=Path, help="Path to after TUI frame text")
    parser.add_argument("--before-label", default="Before", help="Badge label for before panel")
    parser.add_argument("--after-label", default="After", help="Badge label for after panel")
    parser.add_argument("--title", default=None, help="Optional header title above both panels")
    parser.add_argument("--output", type=Path, required=True, help="Output PNG path")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.before and args.after:
        before_img = Image.open(args.before)
        after_img = Image.open(args.after)
    elif args.before_text and args.after_text:
        before_img = render_text_frame(args.before_text.read_text(encoding="utf-8"))
        after_img = render_text_frame(args.after_text.read_text(encoding="utf-8"))
    else:
        print(
            "Provide either (--before and --after) or (--before-text and --after-text)",
            file=sys.stderr,
        )
        return 2

    composed = compose_before_after(
        before_img,
        after_img,
        before_label=args.before_label,
        after_label=args.after_label,
        title=args.title,
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    composed.save(args.output, format="PNG")
    print(f"Wrote {args.output} ({composed.width}x{composed.height})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
