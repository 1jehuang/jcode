#!/usr/bin/env python3
"""UI efficiency analyzer for jcode iOS screenshots.

Turns "this looks ugly" into hill-climbable numbers. Given a PNG screenshot
(from `xcrun simctl io ... screenshot`), it scores the rendered UI on
objective axes and prints a scorecard plus an overall 0-100 efficiency score.

Axes (each 0-100, higher is better):
  space       use of the available canvas: fill ratio, vertical balance,
              and the size of the largest empty "dead zone".
  consistency visual discipline: how few distinct dominant colors, and how
              well content aligns to a small set of left margins.
  legibility  text/background contrast (WCAG-style) for the brightest content.
  rhythm      whether vertical gaps between content rows snap to an 8pt grid.

It is deliberately renderer-agnostic: it reads pixels, not the view tree, so
the same tool grades any screenshot and regressions are caught without trust
in the code. Pair with ui_lint.py (source discipline) for full coverage.

Usage:
  python3 ui_metrics.py SHOT.png [--scale 3] [--json] [--annotate OUT.png]
  python3 ui_metrics.py --baseline a.png --candidate b.png   # compare two
"""

import argparse
import json
import sys
from dataclasses import dataclass, asdict

import numpy as np
from PIL import Image, ImageDraw

# --- Design tokens (must mirror Sources/JCodeMobile/Theme.swift) -------------
TOKENS = {
    "background": 0x25221F,      # Theme.surface: the chat page (Desktop PANEL_BG)
    "surface": 0x1C1A18,         # Theme.background: canvas (Desktop BG)
    "surfaceElevated": 0x302B27, # Desktop HEADER_BG / USER_BG
    "mint": 0xB6A08A,            # key kept for scorer compat: Theme.accent (sandstone)
    "warning": 0xD0B17D,
    "error": 0xE18C85,
}

# iPhone status bar / home indicator are OS chrome, not our UI. Trim them so we
# grade the app's content area, not Apple's clock.
STATUS_BAR_FRAC = 0.055
HOME_INDICATOR_FRAC = 0.025


def hex_to_rgb(h):
    return np.array([(h >> 16) & 0xFF, (h >> 8) & 0xFF, h & 0xFF], dtype=np.float64)


def relative_luminance(rgb):
    srgb = rgb / 255.0
    lin = np.where(srgb <= 0.03928, srgb / 12.92, ((srgb + 0.055) / 1.055) ** 2.4)
    return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]


def contrast_ratio(a, b):
    la, lb = relative_luminance(a), relative_luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


@dataclass
class Scorecard:
    space: float
    consistency: float
    legibility: float
    rhythm: float
    overall: float
    # raw measurements for debugging / regression diffing
    fill_ratio: float
    vertical_balance: float
    dead_zone_frac: float
    dominant_colors: int
    margin_groups: int
    text_contrast: float
    rhythm_snap: float

    def render(self):
        lines = [
            "UI efficiency scorecard",
            "=" * 40,
            f"  space        {bar(self.space)} {self.space:5.1f}",
            f"  consistency  {bar(self.consistency)} {self.consistency:5.1f}",
            f"  legibility   {bar(self.legibility)} {self.legibility:5.1f}",
            f"  rhythm       {bar(self.rhythm)} {self.rhythm:5.1f}",
            "-" * 40,
            f"  OVERALL      {bar(self.overall)} {self.overall:5.1f}",
            "",
            "raw:",
            f"  fill_ratio        {self.fill_ratio:.3f}   (content / canvas)",
            f"  vertical_balance  {self.vertical_balance:.3f}   (1=centered)",
            f"  dead_zone_frac    {self.dead_zone_frac:.3f}   (largest empty band)",
            f"  dominant_colors   {self.dominant_colors}",
            f"  margin_groups     {self.margin_groups}   (distinct left edges)",
            f"  text_contrast     {self.text_contrast:.2f}:1 (WCAG)",
            f"  rhythm_snap       {self.rhythm_snap:.3f}   (gap->8pt grid)",
        ]
        return "\n".join(lines)


def bar(v, width=20):
    filled = int(round(v / 100 * width))
    return "[" + "#" * filled + "." * (width - filled) + "]"


def clamp(v, lo=0.0, hi=100.0):
    return max(lo, min(hi, v))


def analyze(path, scale=3, annotate=None):
    img = Image.open(path).convert("RGB")
    arr = np.asarray(img, dtype=np.float64)
    H, W, _ = arr.shape

    # Trim OS chrome.
    top = int(H * STATUS_BAR_FRAC)
    bot = int(H * (1 - HOME_INDICATOR_FRAC))
    content = arr[top:bot]
    ch, cw, _ = content.shape

    bg = hex_to_rgb(TOKENS["background"])
    # Per-pixel distance from the app background.
    dist = np.linalg.norm(content - bg, axis=2)
    is_content = dist > 18.0  # tolerance for AA + jpeg-ish noise

    # --- SPACE -------------------------------------------------------------
    fill_ratio = float(is_content.mean())

    row_occ = is_content.mean(axis=1)  # fraction of content per row
    ys = np.arange(ch)
    occ_sum = row_occ.sum()
    if occ_sum > 0:
        com = float((ys * row_occ).sum() / occ_sum) / ch  # 0..1 center of mass
    else:
        com = 0.5
    vertical_balance = 1.0 - abs(com - 0.5) * 2.0

    # Largest contiguous "empty" band (rows with <1% content).
    empty = row_occ < 0.01
    dead = longest_run(empty) / ch

    # Space score: reward filling, centering, and penalize a huge dead zone.
    # An ideal chat fills ~35-65% with content reasonably spread.
    fill_score = 100 * (1 - abs(fill_ratio - 0.45) / 0.45)
    space = clamp(0.45 * clamp(fill_score) + 0.35 * (vertical_balance * 100)
                  + 0.20 * (100 * (1 - dead)))

    # --- CONSISTENCY -------------------------------------------------------
    # Distinct dominant colors: quantize to 5 bits/channel, count buckets that
    # cover >0.4% of content pixels. Clean designs use few.
    cont_px = content[is_content]
    if len(cont_px):
        q = (cont_px // 8).astype(np.int64)
        keys = q[:, 0] * 1024 + q[:, 1] * 32 + q[:, 2]
        _, counts = np.unique(keys, return_counts=True)
        dominant = int((counts > 0.004 * len(cont_px)).sum())
    else:
        dominant = 0
    # 4-9 dominant colors is healthy; more = noisy, fewer = empty.
    color_score = 100 * (1 - clamp(abs(dominant - 7) / 12, 0, 1))

    # Left-margin alignment: leftmost content x per row, clustered.
    margins = leftmost_edges(is_content)
    margin_groups = cluster_count(margins, tol=int(8 * scale))
    # A disciplined layout uses 1-3 left margins.
    margin_score = 100 * (1 - clamp((margin_groups - 2) / 6, 0, 1))

    consistency = clamp(0.5 * color_score + 0.5 * margin_score)

    # --- LEGIBILITY --------------------------------------------------------
    # Brightest content (text-ish) contrast vs background.
    if len(cont_px):
        with np.errstate(over="ignore", invalid="ignore", divide="ignore"):
            lum = cont_px @ np.array([0.299, 0.587, 0.114], dtype=np.float64)
            bright = cont_px[lum > np.percentile(lum, 90)]
            sample = bright.mean(axis=0) if len(bright) else cont_px.mean(axis=0)
        text_contrast = contrast_ratio(sample, bg)
    else:
        text_contrast = 1.0
    # WCAG AA body text wants >= 4.5:1; AAA 7:1. Map 1->0, 7->100.
    legibility = clamp((text_contrast - 1.0) / (7.0 - 1.0) * 100)

    # --- RHYTHM ------------------------------------------------------------
    # Gaps between content bands; reward snapping to an 8pt grid.
    bands = content_bands(row_occ, thresh=0.01)
    gaps = [bands[i + 1][0] - bands[i][1] for i in range(len(bands) - 1)]
    grid = 8 * scale
    if gaps:
        snap = np.mean([1 - min(abs((g % grid)), grid - (g % grid)) / (grid / 2)
                        for g in gaps if g > 0]) if any(g > 0 for g in gaps) else 0.0
        snap = float(max(0.0, snap))
    else:
        snap = 0.0
    rhythm = clamp(snap * 100)

    overall = clamp(0.40 * space + 0.30 * consistency
                    + 0.20 * legibility + 0.10 * rhythm)

    card = Scorecard(
        space=round(space, 1), consistency=round(consistency, 1),
        legibility=round(legibility, 1), rhythm=round(rhythm, 1),
        overall=round(overall, 1),
        fill_ratio=round(fill_ratio, 4), vertical_balance=round(vertical_balance, 4),
        dead_zone_frac=round(dead, 4), dominant_colors=dominant,
        margin_groups=margin_groups, text_contrast=round(text_contrast, 2),
        rhythm_snap=round(snap, 4),
    )

    if annotate:
        draw_overlay(img.copy(), top, bot, is_content, bands, annotate, scale)

    return card


def longest_run(boolean_arr):
    best = run = 0
    for v in boolean_arr:
        run = run + 1 if v else 0
        best = max(best, run)
    return best


def leftmost_edges(mask):
    edges = []
    for row in mask:
        idx = np.argmax(row)
        if row[idx]:
            edges.append(int(idx))
    return edges


def cluster_count(values, tol):
    if not values:
        return 0
    vals = sorted(values)
    groups = 1
    anchor = vals[0]
    # weight by frequency: only count a cluster if it has enough rows
    from collections import Counter
    c = Counter(values)
    centers = []
    for v in sorted(c):
        if not centers or v - centers[-1] > tol:
            centers.append(v)
    # keep clusters covering >2% of rows
    total = len(values)
    significant = 0
    bucket = {}
    for v in values:
        placed = False
        for center in centers:
            if abs(v - center) <= tol:
                bucket[center] = bucket.get(center, 0) + 1
                placed = True
                break
    return sum(1 for center, n in bucket.items() if n > 0.02 * total)


def content_bands(row_occ, thresh):
    bands = []
    start = None
    for i, v in enumerate(row_occ):
        if v >= thresh and start is None:
            start = i
        elif v < thresh and start is not None:
            bands.append((start, i))
            start = None
    if start is not None:
        bands.append((start, len(row_occ)))
    # merge tiny gaps
    return [b for b in bands if b[1] - b[0] > 3]


def draw_overlay(img, top, bot, mask, bands, out, scale):
    d = ImageDraw.Draw(img)
    d.rectangle([0, top, img.width - 1, bot], outline=(77, 217, 166), width=2)
    for (s, e) in bands:
        d.rectangle([0, top + s, img.width - 1, top + e],
                    outline=(245, 158, 11), width=1)
    img.save(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path", nargs="?")
    ap.add_argument("--scale", type=int, default=3, help="device px per point")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--annotate", help="write an annotated PNG to this path")
    ap.add_argument("--baseline")
    ap.add_argument("--candidate")
    args = ap.parse_args()

    if args.baseline and args.candidate:
        a = analyze(args.baseline, args.scale)
        b = analyze(args.candidate, args.scale)
        print(f"baseline  overall {a.overall:5.1f}")
        print(f"candidate overall {b.overall:5.1f}")
        delta = b.overall - a.overall
        sign = "+" if delta >= 0 else ""
        print(f"delta     {sign}{delta:.1f}")
        for k in ["space", "consistency", "legibility", "rhythm"]:
            av, bv = getattr(a, k), getattr(b, k)
            dd = bv - av
            s = "+" if dd >= 0 else ""
            print(f"  {k:12} {av:5.1f} -> {bv:5.1f}  ({s}{dd:.1f})")
        sys.exit(0 if delta >= -0.5 else 1)

    if not args.path:
        ap.error("provide a screenshot path, or --baseline/--candidate")

    card = analyze(args.path, args.scale, args.annotate)
    if args.json:
        print(json.dumps(asdict(card), indent=2))
    else:
        print(card.render())


if __name__ == "__main__":
    main()
