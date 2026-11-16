"""Scorer: F. styling.

Grades aesthetic coherence & intent - "designed, not generated". Grounded in
reward/AI_SLOP_RESEARCH.md: crafted UI commits to ONE dominant colour plus a
sharp, sparing accent (not a timid rainbow), uses a real type scale (deliberate
size + weight contrast, not one-size-everywhere), and draws radii from a small
consistent system. Higher = more crafted. Source and pixel are blended so
neither can be gamed alone.

  SOURCE intent (ctx.source_files)
    accent cohesion   Count DISTINCT chromatic accent families referenced across
                      views (Theme.mint/warning/error + any inline Color(hex:)),
                      ignoring neutral surface/text tokens. A committed aesthetic
                      is ONE dominant accent used sparingly (our app: mint
                      #4DD9A6); a "timid rainbow" of many competing accents is
                      slop. Reward few distinct accents AND a high dominance
                      ratio for the leading one.
    type scale        Detect a real scale: >=3 distinct deliberate sizes
                      (Theme.mono(size:) / .system(size:) / semantic font roles)
                      AND weight variation (>=2-3 weights). One size + one weight
                      everywhere is the documented "no real hierarchy" tell.
    radius system     Corner radii should come from a small consistent set.
                      Reward few distinct radii; penalize many arbitrary ones
                      (the opposite of "rounded-2xl on everything", but also the
                      opposite of an unsystematic grab-bag).

  PIXEL corroboration (ctx.content_pixels / content_mask)
    accent sparingness  The accent (mint hue band) should be a SMALL fraction of
                        content pixels - emphasis, not wallpaper. Reward ~1-12%
                        coverage; penalize 0% (no focal accent at all) and >25%
                        (accent overload, the accent stops reading as special).
    surface cohesion    Reward a dominant, cohesive surface palette: most content
                        pixels sit on the tokenized background/surface ramp rather
                        than scattering into many unrelated colours.

Source grades the habits that keep the look intentional across future screens;
pixel grades the look you can actually see. We blend both.
"""

from __future__ import annotations

import re

import numpy as np

from reward.context import Context, TOKENS, hex_to_rgb
from reward.types import CategoryScore, make_unavailable

NAME = "styling"
CATEGORY = "F"
WEIGHT = 0.04

# Neutral Theme tokens (surfaces, borders, text). These are the canvas, not an
# accent, so they never count toward the accent palette.
_NEUTRAL_TOKENS = {
    "background", "surface", "surfaceElevated", "border", "borderStrong",
    "borderFocus", "textPrimary", "textSecondary", "textTertiary", "textUser",
    "toolSurface", "codeBackground", "codeHeader", "inlineCode",
    "inputBackground", "inputBorder", "onAccent",
}
# Chromatic Theme tokens that DO read as accents. mintTint folds into mint (it is
# the same hue at lower alpha, not a second accent).
_ACCENT_TOKEN_FAMILY = {
    "mint": "mint", "mintTint": "mint",
    "accent": "mint", "accentTint": "mint", "link": "mint", "ok": "ok",
    "warning": "warning", "error": "error",
}

_THEME_TOKEN_RE = re.compile(r"Theme\.([A-Za-z]+)")
_INLINE_HEX_RE = re.compile(r"Color\(hex:\s*0x([0-9A-Fa-f]+)\)")
# Deliberate type steps: explicit point sizes + semantic SwiftUI font roles.
_MONO_SIZE_RE = re.compile(r"Theme\.mono\(\s*([0-9]+(?:\.[0-9]+)?)")
_SYSTEM_SIZE_RE = re.compile(r"\.system\(\s*size:\s*([0-9]+(?:\.[0-9]+)?)")
_FONT_ROLE_RE = re.compile(
    r"\.font\(\.(largeTitle|title3|title2|title|headline|subheadline|"
    r"footnote|caption2|caption|callout|body)\b"
)
_WEIGHT_RE = re.compile(r"(?:weight:\s*\.|\.weight\(\.)([a-zA-Z]+)")
_CORNER_RADIUS_RE = re.compile(r"cornerRadius:\s*([0-9]+(?:\.[0-9]+)?)")

# Mint hue (~158 deg) defines the accent band; warning/error are functional
# state colours, not the brand accent we measure for sparingness.
_MINT_HUE = 30.0  # Theme.accent sandstone #B6A08A (Desktop ACCENT)
_HUE_TOL = 26.0


def _base_hue(rgb: np.ndarray) -> float | None:
    """Hue in degrees for an RGB triple, or None for a greyscale colour."""
    a = rgb / 255.0
    mx, mn = float(a.max()), float(a.min())
    d = mx - mn
    if d < 1e-9:
        return None
    r, g, b = a
    if mx == r:
        h = (b - g) / d
    elif mx == g:
        h = 2.0 + (r - b) / d
    else:
        h = 4.0 + (g - r) / d
    return (h / 6.0) % 1.0 * 360.0


def _hsv(arr: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Vectorized HSV (hue degrees, sat 0..1, val 0..1) for an HxWx3 image."""
    a = arr / 255.0
    mx = a.max(axis=-1)
    mn = a.min(axis=-1)
    d = mx - mn
    v = mx
    s = np.where(mx > 1e-9, d / np.maximum(mx, 1e-9), 0.0)
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    rc = (mx - r) / np.maximum(d, 1e-9)
    gc = (mx - g) / np.maximum(d, 1e-9)
    bc = (mx - b) / np.maximum(d, 1e-9)
    h = np.where(mx == r, bc - gc, np.where(mx == g, 2.0 + rc - bc, 4.0 + gc - rc))
    h = np.where(d < 1e-9, 0.0, (h / 6.0) % 1.0 * 360.0)
    return h, s, v


def _band(x: float, lo: float, hi: float, floor_at: float, ceil_at: float) -> float:
    """100 inside [lo, hi]; linear ramp to 0 at floor_at (below) / ceil_at (above)."""
    if lo <= x <= hi:
        return 100.0
    if x < lo:
        return max(0.0, 100.0 * (x - floor_at) / (lo - floor_at)) if lo > floor_at else 0.0
    return max(0.0, 100.0 * (ceil_at - x) / (ceil_at - hi)) if ceil_at > hi else 0.0


def _accent_palette(source_files: dict[str, str]) -> dict[str, int]:
    """Usage count per distinct accent family referenced outside Theme.swift.

    Chromatic Theme tokens collapse to a family (mintTint -> mint); inline
    Color(hex:) literals bucket by hue (15 deg buckets) so two near-identical
    custom colours are not double-counted as a "rainbow".
    """
    usage: dict[str, int] = {}
    for path, text in source_files.items():
        if path.endswith("Theme.swift"):
            continue
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("//"):
                continue
            for tok in _THEME_TOKEN_RE.findall(line):
                if tok in _NEUTRAL_TOKENS:
                    continue
                family = _ACCENT_TOKEN_FAMILY.get(tok)
                if family is not None:
                    usage[family] = usage.get(family, 0) + 1
            for hex_str in _INLINE_HEX_RE.findall(line):
                hue = _base_hue(hex_to_rgb(int(hex_str, 16)))
                if hue is None:
                    continue  # greyscale literal is neutral, not an accent
                key = f"hue{int(round(hue / 15.0) * 15) % 360}"
                usage[key] = usage.get(key, 0) + 1
    return usage


def _type_scale(source_files: dict[str, str]) -> tuple[int, int, int]:
    """(distinct deliberate sizes, distinct semantic roles, distinct weights)."""
    sizes: set[float] = set()
    roles: set[str] = set()
    weights: set[str] = set()
    for text in source_files.values():
        for m in _MONO_SIZE_RE.finditer(text):
            sizes.add(float(m.group(1)))
        for m in _SYSTEM_SIZE_RE.finditer(text):
            sizes.add(float(m.group(1)))
        for m in _FONT_ROLE_RE.finditer(text):
            roles.add(m.group(1))
        for m in _WEIGHT_RE.finditer(text):
            weights.add(m.group(1))
    return len(sizes), len(roles), len(weights)


def _distinct_radii(source_files: dict[str, str]) -> int:
    radii: set[float] = set()
    for text in source_files.values():
        for m in _CORNER_RADIUS_RE.finditer(text):
            radii.add(float(m.group(1)))
    return len(radii)


def score(ctx: Context) -> CategoryScore:
    mask = ctx.content_mask
    source_files = ctx.source_files
    have_pixels = mask is not None
    have_source = bool(source_files)

    if not have_pixels and not have_source:
        return make_unavailable(NAME, CATEGORY, WEIGHT, "no screenshot and no source")

    parts: list[float] = []
    weights: list[float] = []
    evidence: dict = {}

    # --- SOURCE: accent cohesion + type scale + radius system ------------
    if have_source:
        palette = _accent_palette(source_files)
        distinct_accents = len(palette)
        total = sum(palette.values())
        dominance = (max(palette.values()) / total) if total else 0.0
        # 1-3 distinct accents reads as a committed palette; each extra competing
        # accent past 3 is a step toward a timid rainbow.
        spread_score = 100.0 if distinct_accents <= 3 else max(
            0.0, 100.0 - 22.0 * (distinct_accents - 3))
        # One accent doing >=50% of the work is a clear dominant choice.
        dominance_score = min(1.0, dominance / 0.5) * 100.0
        # No accent at all is timid; a single dominant accent is ideal.
        accent_src = 0.6 * spread_score + 0.4 * dominance_score

        type_sizes, type_roles, type_weights = _type_scale(source_files)
        # Effective size tiers include semantic font roles (each maps to a real
        # size) so a view using .caption/.headline is not wrongly read as flat.
        size_tiers = type_sizes + type_roles
        # >=3 deliberate sizes -> full; one-size-everywhere -> 0.
        size_score = min(1.0, max(0.0, (size_tiers - 1) / 2.0)) * 100.0
        # >=3 weights -> full, 2 -> half, 1 (single weight everywhere) -> 0.
        weight_score = min(1.0, max(0.0, (type_weights - 1) / 2.0)) * 100.0
        type_intent = 0.6 * size_score + 0.4 * weight_score

        distinct_radii = _distinct_radii(source_files)
        # <=4 radii is a tidy system; each extra arbitrary radius erodes it.
        radius_score = 100.0 if distinct_radii <= 4 else max(
            0.0, 100.0 - 14.0 * (distinct_radii - 4))

        source_score = 0.4 * accent_src + 0.35 * type_intent + 0.25 * radius_score
        parts.append(source_score)
        weights.append(0.55)
        evidence["distinct_accents"] = int(distinct_accents)
        evidence["accent_dominance"] = round(dominance, 3)
        evidence["type_sizes"] = int(type_sizes)
        evidence["type_roles"] = int(type_roles)
        evidence["type_weights"] = int(type_weights)
        evidence["distinct_radii"] = int(distinct_radii)

    # --- PIXEL: accent sparingness + surface cohesion --------------------
    if have_pixels:
        content = ctx.content_pixels
        h, s, v = _hsv(content)
        # Pixels in the mint hue band with enough saturation/value to be a real
        # accent (not background noise). Measured over non-background content.
        accent = (np.abs(h - _MINT_HUE) <= _HUE_TOL) & (s >= 0.18) & (v >= 0.12)
        nonbg = int(mask.sum())
        accent_frac = float(accent[mask].mean()) if nonbg > 0 else 0.0
        # Emphasis, not wallpaper: reward 1-12%; 0% = no focal accent (ramp from
        # 0), >25% = accent overload (ramp to 0 at 25%).
        accent_score = _band(accent_frac, 0.01, 0.12, 0.0, 0.25)

        surf = np.stack([hex_to_rgb(TOKENS[k])
                         for k in ("background", "surface", "surfaceElevated")])
        cm = content[mask]
        if len(cm) > 0:
            dmin = np.min(np.linalg.norm(cm[:, None, :] - surf[None, :, :], axis=2),
                          axis=1)
            surface_frac = float((dmin < 30.0).mean())
        else:
            surface_frac = 0.0
        # A cohesive look keeps most content on the tokenized surface ramp;
        # ~70%+ reads as one dominant palette.
        surface_score = min(1.0, surface_frac / 0.7) * 100.0

        pixel_score = 0.6 * accent_score + 0.4 * surface_score
        parts.append(pixel_score)
        weights.append(0.45)
        evidence["accent_pixel_frac"] = round(accent_frac, 4)
        evidence["surface_frac"] = round(surface_frac, 4)

    wsum = sum(weights)
    value = sum(p * w for p, w in zip(parts, weights)) / wsum if wsum > 0 else 0.0
    value = max(0.0, min(100.0, value))

    return CategoryScore(
        name=NAME, category=CATEGORY, weight=WEIGHT, value=round(value, 2),
        evidence=evidence,
    )
