#!/usr/bin/env python3
"""Compose a side-by-side Before / After PNG for Discord release announcements.

Accepts either two PNG images (--before / --after) or two plain/ANSI terminal
frame text files (--before-text / --after-text) captured from the previous and
new jcode binaries, and produces a crisp dark-themed comparison card.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

try:
    from PIL import Image, ImageChops, ImageDraw, ImageFont
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
SENSITIVE_PATTERNS: tuple[tuple[re.Pattern[str], str], ...] = (
    # Full or truncated emails (e.g. jeremy@example.com or jeremy@…)
    (re.compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]*[A-Za-z0-9….]*"), "user@example.com"),
    # Home directory paths (/home/<user>/... or /Users/<user>/...)
    (re.compile(r"/(?:home|Users)/[A-Za-z0-9._-]+"), "/home/user"),
    # Common API key / token prefixes
    (re.compile(r"\b(?:sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9]{8,}|AIza[A-Za-z0-9_-]{12,}|xox[baprs]-[A-Za-z0-9-]{8,}|AKIA[0-9A-Z]{12,})\b"), "[REDACTED]"),
)


def sanitize_text(text: str) -> str:
    """Redact emails, home paths, and API tokens while preserving column width where possible."""
    result = text
    for pattern, replacement in SENSITIVE_PATTERNS:
        result = pattern.sub(replacement, result)
    return result


def sanitize_cells_frame(data: dict) -> dict:
    """Return a copy of a `client:cells` frame with emails, home paths, and tokens redacted."""
    clean_rows = []
    for row in data.get("rows") or []:
        clean_row = []
        for run in row:
            if isinstance(run, list) and len(run) >= 4 and isinstance(run[0], str):
                orig = run[0]
                redacted = sanitize_text(orig)
                if len(redacted) < len(orig):
                    redacted = redacted + (" " * (len(orig) - len(redacted)))
                elif len(redacted) > len(orig):
                    redacted = redacted[: len(orig)]
                clean_row.append([redacted, run[1], run[2], run[3]])
            else:
                clean_row.append(run)
        clean_rows.append(clean_row)
    return {
        "width": data.get("width", 80),
        "height": data.get("height", len(clean_rows)),
        "rows": clean_rows,
    }
FONT_CANDIDATES = (
    "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf",
    "/usr/share/fonts/TTF/JetBrainsMonoNLNerdFont-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
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


def crop_cells_rows(data: dict, spec: str | None) -> dict:
    """Slice a `client:cells` payload to a 0-based `start:end` row range."""
    if not spec:
        return data
    start_s, _, end_s = spec.partition(":")
    rows = data.get("rows") or []
    start = max(0, int(start_s)) if start_s.strip() else 0
    end = min(len(rows), int(end_s)) if end_s.strip() else len(rows)
    sliced = rows[start:end]
    return {
        "width": data.get("width", 80),
        "height": len(sliced),
        "rows": sliced,
    }


def render_cells_frame(
    data: dict,
    *,
    font_size: int = 26,
    padding: int = 24,
) -> Image.Image:
    """Render a jcode `client:cells` JSON frame with exact RGB colors and styles."""
    width_cells = max(1, int(data.get("width", 80)))
    height_cells = max(1, int(data.get("height", 24)))
    rows = data.get("rows") or []

    font = load_mono_font(font_size)
    bold_font = font
    italic_font = font
    for candidate in (
        "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Bold.ttf",
        "/usr/share/fonts/noto/NotoSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
    ):
        if Path(candidate).is_file():
            try:
                bold_font = ImageFont.truetype(candidate, size=font_size)
                break
            except OSError:
                continue
    for candidate in (
        "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Italic.ttf",
        "/usr/share/fonts/TTF/JetBrainsMonoNLNerdFont-Italic.ttf",
    ):
        if Path(candidate).is_file():
            try:
                italic_font = ImageFont.truetype(candidate, size=font_size)
                break
            except OSError:
                continue

    cell_w = max(8, int(round(font.getlength("M"))))
    cell_h = max(font_size + 3, int(round(font_size * 1.18)))

    img_w = width_cells * cell_w + padding * 2
    img_h = height_cells * cell_h + padding * 2
    default_bg = (0, 0, 0)
    default_fg = TEXT_COLOR

    img = Image.new("RGB", (img_w, img_h), default_bg)
    draw = ImageDraw.Draw(img)

    # Pass 1: paint all cell backgrounds first so descenders from row N are
    # never clipped by row N+1's background rectangle.
    parsed_rows: list[list[tuple[int, str, tuple[int, int, int], tuple[int, int, int], int]]] = []
    for row_idx, row in enumerate(rows[:height_cells]):
        y = padding + row_idx * cell_h
        col = 0
        parsed_row = []
        for run in row:
            if not isinstance(run, list) or len(run) < 4:
                continue
            text, fg_raw, bg_raw, flags = run[0], run[1], run[2], int(run[3])
            fg = tuple(fg_raw) if fg_raw is not None else default_fg
            bg = tuple(bg_raw) if bg_raw is not None else default_bg
            if flags & 16:  # REVERSED
                fg, bg = bg, fg
            if flags & 4:  # DIM
                fg = tuple((c * 55 + b * 45) // 100 for c, b in zip(fg, bg))
            x0 = padding + col * cell_w
            run_cols = len(text)
            if bg != default_bg and run_cols > 0:
                draw.rectangle([x0, y, x0 + run_cols * cell_w, y + cell_h], fill=bg)
            parsed_row.append((col, text, fg, bg, flags))
            col += run_cols
        parsed_rows.append(parsed_row)

    # Fallback font for symbols missing from JetBrainsMono Nerd Font (e.g. ★).
    symbol_font = font
    for candidate in (
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
    ):
        if Path(candidate).is_file():
            try:
                symbol_font = ImageFont.truetype(candidate, size=font_size)
                break
            except OSError:
                continue

    # Pass 2: draw glyphs, crisp vector parallelograms (▰/▱), and cell-aligned
    # box-drawing / symbol glyphs so corners join cleanly and ★ never tofu-boxes.
    special_chars = set("▰▱★☆─│╭╮╰╯┌┐└┘")
    for row_idx, parsed_row in enumerate(parsed_rows):
        y = padding + row_idx * cell_h
        for col, text, fg, _bg, flags in parsed_row:
            if flags & 1:
                run_font = bold_font
            elif flags & 2:
                run_font = italic_font
            else:
                run_font = font
            x0 = padding + col * cell_w
            run_cols = len(text)
            if any(ch in special_chars for ch in text):
                for i, ch in enumerate(text):
                    cx = x0 + i * cell_w
                    if ch in ("▰", "▱"):
                        skew = max(2, cell_w // 5)
                        pad_x = max(1, cell_w // 10)
                        pad_y = max(4, cell_h // 4)
                        poly = [
                            (cx + pad_x + skew, y + pad_y),
                            (cx + cell_w - pad_x, y + pad_y),
                            (cx + cell_w - pad_x - skew, y + cell_h - pad_y),
                            (cx + pad_x, y + cell_h - pad_y),
                        ]
                        if ch == "▰":
                            draw.polygon(poly, fill=fg)
                        else:
                            draw.polygon(poly, outline=fg, width=2)
                    elif ch in ("★", "☆"):
                        draw.text((cx, y + 1), ch, fill=fg, font=symbol_font)
                    elif ch == "─":
                        mid_y = y + cell_h // 2
                        draw.line([cx, mid_y, cx + cell_w, mid_y], fill=fg, width=2)
                    elif ch == "│":
                        mid_x = cx + cell_w // 2
                        draw.line([mid_x, y, mid_x, y + cell_h], fill=fg, width=2)
                    elif ch in ("╭", "┌"):
                        mid_x = cx + cell_w // 2
                        mid_y = y + cell_h // 2
                        draw.line([mid_x, mid_y, cx + cell_w, mid_y], fill=fg, width=2)
                        draw.line([mid_x, mid_y, mid_x, y + cell_h], fill=fg, width=2)
                    elif ch in ("╮", "┐"):
                        mid_x = cx + cell_w // 2
                        mid_y = y + cell_h // 2
                        draw.line([cx, mid_y, mid_x, mid_y], fill=fg, width=2)
                        draw.line([mid_x, mid_y, mid_x, y + cell_h], fill=fg, width=2)
                    elif ch in ("╰", "└"):
                        mid_x = cx + cell_w // 2
                        mid_y = y + cell_h // 2
                        draw.line([mid_x, y, mid_x, mid_y], fill=fg, width=2)
                        draw.line([mid_x, mid_y, cx + cell_w, mid_y], fill=fg, width=2)
                    elif ch in ("╯", "┘"):
                        mid_x = cx + cell_w // 2
                        mid_y = y + cell_h // 2
                        draw.line([mid_x, y, mid_x, mid_y], fill=fg, width=2)
                        draw.line([cx, mid_y, mid_x, mid_y], fill=fg, width=2)
                    elif ch != " ":
                        draw.text((cx, y + 1), ch, fill=fg, font=run_font)
            elif text.strip():
                draw.text((x0, y + 1), text, fill=fg, font=run_font)
            if (flags & 8) and run_cols > 0:  # UNDERLINED
                draw.line([x0, y + cell_h - 3, x0 + run_cols * cell_w, y + cell_h - 3], fill=fg, width=2)

    return img


def compute_diff_bbox(
    before: Image.Image,
    after: Image.Image,
    *,
    threshold: int = 18,
) -> tuple[int, int, int, int] | None:
    """Return the (x0, y0, x1, y1) bounding box where before and after differ."""
    w = min(before.width, after.width)
    h = min(before.height, after.height)
    if w <= 0 or h <= 0:
        return None
    b = before.crop((0, 0, w, h)).convert("RGB")
    a = after.crop((0, 0, w, h)).convert("RGB")
    diff = ImageChops.difference(b, a).convert("L")
    mask = diff.point(lambda v: 255 if v >= threshold else 0)
    return mask.getbbox()


def compose_before_after(
    before_img: Image.Image,
    after_img: Image.Image,
    *,
    before_label: str = "Before",
    after_label: str = "After",
    title: str | None = None,
    highlight_diff: bool = True,
    auto_crop: bool = False,
    crop_pad: int = 48,
) -> Image.Image:
    before = before_img.convert("RGB")
    after = after_img.convert("RGB")

    # Preserve identical 1:1 scale when heights differ by padding the shorter
    # panel vertically (top-aligned) instead of distorting font sizes.
    target_h = max(before.height, after.height)
    if before.height != target_h:
        padded = Image.new("RGB", (before.width, target_h), before.getpixel((0, 0)))
        padded.paste(before, (0, 0))
        before = padded
    if after.height != target_h:
        padded = Image.new("RGB", (after.width, target_h), after.getpixel((0, 0)))
        padded.paste(after, (0, 0))
        after = padded

    diff_bbox = compute_diff_bbox(before, after) if (highlight_diff or auto_crop) else None
    if auto_crop and diff_bbox is not None:
        x0, y0, x1, y1 = diff_bbox
        cx0 = max(0, x0 - crop_pad)
        cy0 = max(0, y0 - crop_pad)
        cx1 = min(min(before.width, after.width), x1 + crop_pad)
        cy1 = min(min(before.height, after.height), y1 + crop_pad)
        before = before.crop((cx0, cy0, cx1, cy1))
        after = after.crop((cx0, cy0, cx1, cy1))
        target_h = max(before.height, after.height)
        diff_bbox = (x0 - cx0, y0 - cy0, x1 - cx0, y1 - cy0)

    ui_scale = max(1.0, min(2.0, target_h / 650.0))
    outer_pad = round(24 * ui_scale)
    gap = round(20 * ui_scale)
    card_pad = round(10 * ui_scale)
    badge_h = round(34 * ui_scale)
    title_h = round(38 * ui_scale) if title else 0

    card_w_before = before.width + card_pad * 2
    card_w_after = after.width + card_pad * 2
    card_h = badge_h + target_h + card_pad * 2

    total_w = outer_pad * 2 + card_w_before + gap + card_w_after
    total_h = outer_pad * 2 + title_h + card_h

    canvas = Image.new("RGB", (total_w, total_h), BG_COLOR)
    draw = ImageDraw.Draw(canvas)
    title_font = load_mono_font(round(18 * ui_scale))
    badge_font = load_mono_font(round(14 * ui_scale))

    top_y = outer_pad
    if title:
        draw.text((outer_pad, top_y + round(4 * ui_scale)), title, fill=TITLE_COLOR, font=title_font)
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
            radius=round(10 * ui_scale),
            fill=CARD_BG,
            outline=CARD_BORDER,
            width=max(1, round(ui_scale)),
        )
        pill_x0 = x + card_pad
        pill_y0 = top_y + round(7 * ui_scale)
        text_box = badge_font.getbbox(label)
        pill_w = (text_box[2] - text_box[0]) + round(18 * ui_scale)
        pill_h = round(24 * ui_scale)
        draw.rounded_rectangle(
            [pill_x0, pill_y0, pill_x0 + pill_w, pill_y0 + pill_h],
            radius=round(6 * ui_scale),
            fill=badge_bg,
        )
        draw.text(
            (pill_x0 + round(9 * ui_scale), pill_y0 + round(3 * ui_scale)),
            label,
            fill=badge_fg,
            font=badge_font,
        )
        img_x0 = x + card_pad
        img_y0 = top_y + badge_h + card_pad
        canvas.paste(img, (img_x0, img_y0))
        if highlight_diff and diff_bbox is not None:
            dx0, dy0, dx1, dy1 = diff_bbox
            pad = 6
            bx0 = img_x0 + max(0, dx0 - pad)
            by0 = img_y0 + max(0, dy0 - pad)
            bx1 = img_x0 + min(img.width - 1, dx1 + pad)
            by1 = img_y0 + min(img.height - 1, dy1 + pad)
            box_width = max(2, min(4, target_h // 250 + 2))
            draw.rounded_rectangle(
                [bx0, by0, bx1, by1],
                radius=6,
                outline=badge_fg,
                width=box_width,
            )

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
    parser.add_argument("--before-cells", type=Path, help="Path to before `client:cells` JSON")
    parser.add_argument("--after-cells", type=Path, help="Path to after `client:cells` JSON")
    parser.add_argument("--before-crop-rows", default=None, help="Optional 0-based start:end row slice for before cells")
    parser.add_argument("--after-crop-rows", default=None, help="Optional 0-based start:end row slice for after cells")
    parser.add_argument("--crop-rows", default=None, help="Optional 0-based start:end row slice for --cells")
    parser.add_argument("--cells", type=Path, help="Render a single `client:cells` JSON to PNG")
    parser.add_argument("--before-label", default="Before", help="Badge label for before panel")
    parser.add_argument("--after-label", default="After", help="Badge label for after panel")
    parser.add_argument("--title", default=None, help="Optional header title above both panels")
    parser.add_argument(
        "--no-highlight-diff",
        action="store_true",
        help="Disable drawing red/green highlight boxes around the changed region",
    )
    parser.add_argument(
        "--auto-crop",
        action="store_true",
        help="Auto-crop both panels around the changed region with padding",
    )
    parser.add_argument(
        "--sanitize",
        action="store_true",
        help="Redact emails, home directory usernames, and API key prefixes in cell captures",
    )
    parser.add_argument("--output", type=Path, required=True, help="Output PNG path")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.cells:
        data = crop_cells_rows(
            json.loads(args.cells.read_text(encoding="utf-8")),
            args.crop_rows,
        )
        if args.sanitize:
            data = sanitize_cells_frame(data)
        rendered = render_cells_frame(data)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        rendered.save(args.output, format="PNG")
        print(f"Wrote {args.output} ({rendered.width}x{rendered.height})")
        return 0
    if args.before and args.after:
        before_img = Image.open(args.before)
        after_img = Image.open(args.after)
    elif args.before_cells and args.after_cells:
        before_data = crop_cells_rows(
            json.loads(args.before_cells.read_text(encoding="utf-8")),
            args.before_crop_rows,
        )
        after_data = crop_cells_rows(
            json.loads(args.after_cells.read_text(encoding="utf-8")),
            args.after_crop_rows,
        )
        if args.sanitize:
            before_data = sanitize_cells_frame(before_data)
            after_data = sanitize_cells_frame(after_data)
        before_img = render_cells_frame(before_data)
        after_img = render_cells_frame(after_data)
    elif args.before_text and args.after_text:
        before_img = render_text_frame(args.before_text.read_text(encoding="utf-8"))
        after_img = render_text_frame(args.after_text.read_text(encoding="utf-8"))
    else:
        print(
            "Provide --cells, (--before and --after), (--before-cells and --after-cells), or (--before-text and --after-text)",
            file=sys.stderr,
        )
        return 2

    composed = compose_before_after(
        before_img,
        after_img,
        before_label=args.before_label,
        after_label=args.after_label,
        title=args.title,
        highlight_diff=not args.no_highlight_diff,
        auto_crop=args.auto_crop,
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    composed.save(args.output, format="PNG")
    print(f"Wrote {args.output} ({composed.width}x{composed.height})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
