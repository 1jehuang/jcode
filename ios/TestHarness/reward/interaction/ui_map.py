"""Map the SwiftUI source -> tappable-control GEOMETRY per screen (`ui_map` worker).

This module extracts the geometry (size + on-screen center, in points) of every
interactive control on each jcode-mobile screen so the interaction-cost model can
apply Fitts' law to *real* targets. It is the `ui_map` piece described in the
package docstring and depends only on the shared contract in ``model.py`` plus the
standard library (NumPy/PIL are imported lazily, only by the optional
screenshot-based variant).

How geometry is derived
-----------------------
1. SIZE. We parse the Swift views for explicit ``.frame(width: W, height: H)``
   declarations attached to ``Image``/``Button`` controls (e.g. the 44x44 send,
   stop, and settings buttons in ``ChatView.swift``). These are read straight
   from source, so when someone re-sizes the send button the map follows. For
   controls without an explicit frame (the composer text field, list rows, the
   Pair / Scan-QR buttons) we INFER the size from the control's parsed
   ``.padding(...)`` plus the line height of its SwiftUI font:
       control_height ~= font_line_height + 2 * vertical_padding
   Font line heights use a documented Apple-text-style table (see ``_FONT_LINE``)
   and ``Theme.mono(n)`` ~= round(n * 1.30).

2. POSITION. We place controls using known SwiftUI layout rules:
   - The chat HEADER is pinned to the top, just under the device safe-area inset;
     its trailing settings button sits ``H_MARGIN`` from the right edge.
   - The chat COMPOSER is pinned to the bottom, just above the home-indicator
     safe-area inset; the send button is the trailing control, the (conditional)
     stop button sits one ``ICON_GAP`` to its left, and the text field fills the
     remaining width.
   - The SETTINGS sheet is a grouped ``List`` inside a ``NavigationStack``; rows
     are stacked top-down (section header + N rows + inter-section gap) below the
     inline nav bar (which carries the trailing "Done" button).
   - The PAIRING screen is a ``ScrollView`` ``VStack(spacing: 20)``; controls are
     stacked top-down from the content's at-rest (scroll offset 0) origin.

Every non-obvious number is a named, documented constant, and the assumptions are
also returned programmatically by :func:`assumptions` / :data:`LAYOUT_ASSUMPTIONS`
so a reader (or the cost model) can audit exactly what was measured vs assumed.

Determinism
-----------
``build_ui_map`` is pure aside from reading the Swift source files; given the same
sources + screen size it always returns identical geometry. The optional
``build_ui_map_from_screenshot`` is likewise deterministic (fixed color/threshold
math over the pixels).
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from .model import UITarget

# ---------------------------------------------------------------------------
# Device + iOS layout constants (documented; override screen size via args).
# ---------------------------------------------------------------------------

# iPhone 17 logical portrait canvas (the harness's reference device).
DEFAULT_SCREEN_W_PT: float = 402.0
DEFAULT_SCREEN_H_PT: float = 874.0

# Safe-area insets for a Dynamic-Island-class device in portrait (iPhone 14
# Pro/15/16/17 share ~59pt top / ~34pt bottom). The header is pinned below the
# top inset; the composer is pinned above the bottom (home-indicator) inset.
TOP_SAFE_INSET_PT: float = 59.0
BOTTOM_SAFE_INSET_PT: float = 34.0

# Apple HIG minimum hit target; used as a fallback when source has no frame.
HIG_MIN_PT: float = 44.0

# Standard iOS metrics (documented defaults).
NAV_BAR_H_PT: float = 44.0          # inline navigation bar height
LIST_ROW_H_PT: float = 44.0         # single-line grouped list row
LIST_ROW_2LINE_H_PT: float = 60.0   # two-line row (server name + host:port)
SECTION_HEADER_H_PT: float = 28.0   # grouped section header ("Model", ...)
SECTION_GAP_PT: float = 20.0        # vertical gap between grouped sections
LIST_TOP_PAD_PT: float = 16.0       # inset before the first section header
LIST_H_MARGIN_PT: float = 16.0      # grouped list horizontal margin
# A default large-detent sheet leaves a small gap at the very top (the parent
# peeks behind the rounded card); the sheet's nav bar begins about here.
SHEET_TOP_PT: float = 24.0

# Representative counts for source-driven *dynamic* lists (models/sessions/
# servers come from runtime state, not source). Chosen small + fixed so the map
# stays deterministic; the assumption is recorded in LAYOUT_ASSUMPTIONS.
N_MODEL_ROWS: int = 3
N_SESSION_ROWS: int = 3
N_SERVER_ROWS: int = 2

# Approximate "Done" nav-button text width (pt) for its trailing hit box.
NAV_DONE_W_PT: float = 54.0

# ---------------------------------------------------------------------------
# Font line heights (pt). Apple text-style ~line-heights + a mono multiplier.
# ---------------------------------------------------------------------------

_FONT_LINE: dict[str, float] = {
    "caption2": 13.0,
    "caption": 16.0,
    "footnote": 18.0,
    "subheadline": 20.0,
    "callout": 21.0,
    "body": 22.0,
    "headline": 22.0,
    "title3": 25.0,
    "title": 41.0,
}
_MONO_LINE_MULT: float = 1.30  # Theme.mono(n) line height ~= round(n * 1.30)


def _mono_line(size: float) -> float:
    return round(size * _MONO_LINE_MULT)


# ---------------------------------------------------------------------------
# Source parsing helpers (deterministic regex over the Swift view files).
# ---------------------------------------------------------------------------

_FILES = ("ChatView.swift", "PairingView.swift", "SettingsView.swift", "RootView.swift")

# Image(systemName: "x") ... .frame(width: W, height: H)  (W/H may be ints/floats)
_IMG_RE = re.compile(r'Image\(systemName:\s*"([^"]+)"\)')
_FRAME_RE = re.compile(r"\.frame\(\s*width:\s*([0-9.]+)\s*,\s*height:\s*([0-9.]+)\s*\)")
_PAD_AXIS_RE = r"\.padding\(\.{axis},\s*([0-9.]+)\)"
_PAD_ALL_RE = re.compile(r"\.padding\(\s*([0-9.]+)\s*\)")


def _read_sources(source_root: str) -> dict[str, str]:
    """Read the Swift view files; missing files map to '' (defaults kick in)."""
    root = Path(source_root) / "Views"
    out: dict[str, str] = {}
    for name in _FILES:
        p = root / name
        try:
            out[name] = p.read_text(encoding="utf-8")
        except OSError:
            out[name] = ""
    return out


def _image_frames(text: str) -> dict[str, tuple[float, float]]:
    """Map each ``Image(systemName:)`` to the first ``.frame(width:height:)`` that
    follows it (before the next image), i.e. the icon's explicit size from source."""
    frames: dict[str, tuple[float, float]] = {}
    images = list(_IMG_RE.finditer(text))
    for i, m in enumerate(images):
        name = m.group(1)
        end = images[i + 1].start() if i + 1 < len(images) else len(text)
        fm = _FRAME_RE.search(text, m.end(), end)
        if fm:
            frames[name] = (float(fm.group(1)), float(fm.group(2)))
    return frames


def _pad_after(text: str, anchor: str, axis: str, default: float) -> float:
    """First ``.padding(.{axis}, N)`` after ``anchor``; else ``default``."""
    idx = text.find(anchor)
    if idx == -1:
        return default
    m = re.search(_PAD_AXIS_RE.format(axis=axis), text[idx:])
    return float(m.group(1)) if m else default


def _pad_all_after(text: str, anchor: str, default: float) -> float:
    """First ``.padding(N)`` (all-edges) after ``anchor``; else ``default``."""
    idx = text.find(anchor)
    if idx == -1:
        return default
    m = _PAD_ALL_RE.search(text[idx:])
    return float(m.group(1)) if m else default


# Records, per target id, whether the SIZE came from a parsed source frame or a
# documented default. Populated during build for auditability.
LAYOUT_ASSUMPTIONS: dict[str, str] = {}


def _note(target_id: str, msg: str) -> None:
    LAYOUT_ASSUMPTIONS[target_id] = msg


def _size_from_source(
    frames: dict[str, tuple[float, float]], system_name: str, target_id: str
) -> tuple[float, float]:
    """Return the parsed (w, h) for an icon, or the HIG-min fallback + a note."""
    if system_name in frames:
        return frames[system_name]
    _note(target_id, f"size: no .frame for Image({system_name!r}); assumed {HIG_MIN_PT}x{HIG_MIN_PT} (HIG min)")
    return (HIG_MIN_PT, HIG_MIN_PT)


# ---------------------------------------------------------------------------
# Per-screen builders.
# ---------------------------------------------------------------------------

def _chat_targets(
    src: dict[str, str], screen_w: float, screen_h: float
) -> list[UITarget]:
    """Chat screen: header (top, pinned) + composer (bottom, pinned)."""
    chat = src.get("ChatView.swift", "")
    frames = _image_frames(chat)

    h_margin = _pad_after(chat, "private var header", "horizontal", 16.0)
    header_vpad = _pad_after(chat, "private var header", "vertical", 8.0)
    comp_vpad = _pad_after(chat, "HStack(alignment: .bottom", "vertical", 8.0)
    comp_hmargin = _pad_after(chat, "HStack(alignment: .bottom", "horizontal", 16.0)
    # Text-field inner vertical padding (drives its single-line height).
    field_vpad = _pad_after(chat, "text: $draft", "vertical", 8.0)
    icon_gap = 8.0  # HStack(spacing: 8) in both header and composer

    targets: list[UITarget] = []

    # --- Settings (ellipsis) button: trailing item of the top-pinned header ---
    sw, sh = _size_from_source(frames, "ellipsis.circle", "settings")
    header_band_top = TOP_SAFE_INSET_PT
    settings_x = screen_w - h_margin - sw / 2.0
    settings_y = header_band_top + header_vpad + sh / 2.0
    targets.append(UITarget("settings", sw, sh, settings_x, settings_y, exists=True))

    # --- Composer band, pinned to the bottom above the home indicator ---------
    band_bottom = screen_h - BOTTOM_SAFE_INSET_PT
    btn_bottom = band_bottom - comp_vpad  # 44pt buttons sit on the band's bottom

    # Send button (trailing, always present; just disabled when nothing to send).
    snd_w, snd_h = _size_from_source(frames, "arrow.up", "send")
    send_x = screen_w - comp_hmargin - snd_w / 2.0
    send_y = btn_bottom - snd_h / 2.0
    targets.append(UITarget("send", snd_w, snd_h, send_x, send_y, exists=True))

    # Stop / interrupt button: rendered ONLY while processing -> exists=False but
    # still mapped so the cost model can price the processing state too.
    st_w, st_h = _size_from_source(frames, "stop.fill", "stop")
    send_left = send_x - snd_w / 2.0
    stop_x = (send_left - icon_gap) - st_w / 2.0
    stop_y = btn_bottom - st_h / 2.0
    targets.append(UITarget("stop", st_w, st_h, stop_x, stop_y, exists=False))
    _note("stop", "exists=False: only shown while model.session.isProcessing")

    # Composer text field: fills the width left of the trailing buttons. In the
    # default (idle) chat state the stop button is absent, so the field extends
    # to just left of the send button.
    field_line = _FONT_LINE["body"]  # TextField uses .font(.body)
    field_h = field_line + 2.0 * field_vpad
    field_left = comp_hmargin
    field_right = send_left - icon_gap
    field_w = field_right - field_left
    field_x = (field_left + field_right) / 2.0
    field_y = btn_bottom - field_h / 2.0  # HStack(alignment: .bottom)
    targets.append(UITarget("composer_field", field_w, field_h, field_x, field_y, exists=True))
    _note("composer_field", f"height = body line({field_line}) + 2*vpad({field_vpad}); single-line at rest")

    # --- Conditional in-transcript targets -----------------------------------
    # These exist only for specific content states, so they are mapped with
    # exists=False (the engine forces them present for the actions that use
    # them; see engine._targets_for_action). Positions are representative:
    # a tool card header sits in the middle band of the transcript; the
    # jump-to-latest button is pinned bottom-trailing above the composer; a
    # notice dismiss button sits under the header band.
    transcript_top = header_band_top + header_vpad * 2.0 + 44.0
    transcript_bottom = band_bottom - comp_vpad * 2.0 - 44.0
    mid_y = (transcript_top + transcript_bottom) / 2.0

    card_w = screen_w - 2.0 * h_margin
    targets.append(UITarget("tool_card_header", card_w, 33.0, screen_w / 2.0, mid_y, exists=False))
    _note("tool_card_header", "exists=False: only with tool calls; height = mono(13) line + 2*8 padding; centered in transcript band")

    targets.append(UITarget("reasoning_row", card_w, 18.0, screen_w / 2.0, mid_y - 60.0, exists=False))
    _note("reasoning_row", "exists=False: only while reasoning text present; one mono(12) line collapsed")

    jump_w, jump_h = _size_from_source(frames, "arrow.down", "jump_to_latest")
    targets.append(
        UITarget("jump_to_latest", jump_w, jump_h,
                 screen_w - h_margin - jump_w / 2.0,
                 transcript_bottom - jump_h / 2.0, exists=False))
    _note("jump_to_latest", "exists=False: only when scrolled up; bottom-trailing above the composer")

    targets.append(
        UITarget("notice_dismiss", HIG_MIN_PT, HIG_MIN_PT,
                 screen_w - h_margin - HIG_MIN_PT / 2.0,
                 transcript_top + HIG_MIN_PT / 2.0, exists=False))
    _note("notice_dismiss", "exists=False: only with an active notice; trailing 44pt xmark under the header")

    return targets


def _settings_targets(
    src: dict[str, str], screen_w: float, screen_h: float
) -> list[UITarget]:
    """Settings sheet: NavigationStack + grouped List (Model/Sessions/Servers)."""
    targets: list[UITarget] = []
    row_x = screen_w / 2.0
    row_w = screen_w - 2.0 * LIST_H_MARGIN_PT

    # "Done": trailing confirmationAction in the inline nav bar.
    done_x = screen_w - LIST_H_MARGIN_PT - NAV_DONE_W_PT / 2.0
    done_y = SHEET_TOP_PT + NAV_BAR_H_PT / 2.0
    targets.append(UITarget("settings_done", NAV_DONE_W_PT, NAV_BAR_H_PT, done_x, done_y, exists=True))
    _note("settings_done", f"sheet top assumed {SHEET_TOP_PT}pt (large-detent gap); 'Done' text width ~{NAV_DONE_W_PT}pt")

    # Walk the list top-down.
    y = SHEET_TOP_PT + NAV_BAR_H_PT + LIST_TOP_PAD_PT

    def add_rows(prefix: str, n: int, row_h: float) -> None:
        nonlocal y
        for i in range(n):
            cy = y + row_h / 2.0
            targets.append(UITarget(f"{prefix}_{i}", row_w, row_h, row_x, cy, exists=True))
            y += row_h

    def add_button(tid: str) -> None:
        nonlocal y
        cy = y + LIST_ROW_H_PT / 2.0
        targets.append(UITarget(tid, row_w, LIST_ROW_H_PT, row_x, cy, exists=True))
        y += LIST_ROW_H_PT

    # Section: Model
    y += SECTION_HEADER_H_PT
    add_rows("model_row", N_MODEL_ROWS, LIST_ROW_H_PT)
    _note("model_row_*", f"{N_MODEL_ROWS} representative rows (count is runtime/dynamic); {LIST_ROW_H_PT}pt each")

    # Section: Sessions (rows + "Rename" + "New session")
    y += SECTION_GAP_PT + SECTION_HEADER_H_PT
    add_rows("session_row", N_SESSION_ROWS, LIST_ROW_H_PT)
    _note("session_row_*", f"{N_SESSION_ROWS} representative rows (count is runtime/dynamic); {LIST_ROW_H_PT}pt each")
    add_button("rename_session")
    add_button("new_session")

    # Section: Servers (two-line rows + "Pair new server")
    y += SECTION_GAP_PT + SECTION_HEADER_H_PT
    add_rows("server_row", N_SERVER_ROWS, LIST_ROW_2LINE_H_PT)
    _note("server_row_*", f"{N_SERVER_ROWS} representative two-line rows ({LIST_ROW_2LINE_H_PT}pt); count is runtime/dynamic")
    add_button("pair_new_server")

    return targets


def _pairing_targets(
    src: dict[str, str], screen_w: float, screen_h: float
) -> list[UITarget]:
    """Pairing screen: ScrollView VStack(spacing: 20), stacked from the top."""
    pair = src.get("PairingView.swift", "")
    vstack_pad = 16.0       # outer .padding(16)
    vstack_spacing = 20.0   # VStack(alignment:.leading, spacing: 20)
    header_top_pad = _pad_after(pair, "private var header", "top", 32.0)
    # Card lives in Theme.swift (.padding(14)); documented default if absent.
    card_pad = 14.0
    field_pad = _pad_all_after(pair, "private func field", 12.0)
    pair_vpad = _pad_after(pair, "Text(isPairing", "vertical", 16.0)
    scan_vpad = _pad_after(pair, "qrcode.viewfinder", "vertical", 12.0)

    targets: list[UITarget] = []
    content_w = screen_w - 2.0 * vstack_pad
    full_x = screen_w / 2.0

    # Header (title + subtitle), not interactive -> only used to advance y.
    y = TOP_SAFE_INSET_PT + vstack_pad + header_top_pad
    title_line = _mono_line(34.0)
    subtitle_line = _FONT_LINE["subheadline"]
    header_h = title_line + 8.0 + subtitle_line  # internal spacing 8
    y += header_h + vstack_spacing

    # Card with three text fields.
    field_line = _mono_line(16.0)            # TextField uses Theme.mono(16)
    field_h = field_line + 2.0 * field_pad
    label_line = _FONT_LINE["caption"]       # field label uses .font(.caption)
    field_block_h = label_line + 8.0 + field_h  # VStack(spacing: 8)
    card_content_w = content_w - 2.0 * card_pad
    field_x = vstack_pad + card_pad + card_content_w / 2.0

    card_content_top = y + card_pad
    for i, name in enumerate(("host", "port", "code")):
        block_top = card_content_top + i * (field_block_h + 16.0)  # inner spacing 16
        tf_top = block_top + label_line + 8.0
        cy = tf_top + field_h / 2.0
        targets.append(UITarget(f"pairing_field_{name}", card_content_w, field_h, field_x, cy, exists=True))
    _note("pairing_field_*", f"height = mono(16) line({field_line}) + 2*pad({field_pad})")

    card_h = 2.0 * card_pad + 3.0 * field_block_h + 2.0 * 16.0
    y += card_h + vstack_spacing

    # Pair button (full width, prominent).
    pair_h = _FONT_LINE["headline"] + 2.0 * pair_vpad
    targets.append(UITarget("pair_button", content_w, pair_h, full_x, y + pair_h / 2.0, exists=True))
    _note("pair_button", f"height = headline line({_FONT_LINE['headline']}) + 2*vpad({pair_vpad})")
    y += pair_h + vstack_spacing

    # Scan QR button (full width, secondary).
    scan_h = _FONT_LINE["subheadline"] + 2.0 * scan_vpad
    targets.append(UITarget("scan_qr", content_w, scan_h, full_x, y + scan_h / 2.0, exists=True))
    _note("scan_qr", f"height = subheadline line({_FONT_LINE['subheadline']}) + 2*vpad({scan_vpad})")

    return targets


# ---------------------------------------------------------------------------
# Public API.
# ---------------------------------------------------------------------------

def _default_source_root() -> str:
    """<repo>/ios/Sources/JCodeMobile, resolved relative to this file."""
    return str(Path(__file__).resolve().parents[3] / "Sources" / "JCodeMobile")


def build_ui_map(
    source_root: str = "",
    screen_w_pt: float = DEFAULT_SCREEN_W_PT,
    screen_h_pt: float = DEFAULT_SCREEN_H_PT,
) -> dict[str, list[UITarget]]:
    """Build the per-screen map of tappable-control geometry from the Swift source.

    Returns a dict keyed by screen/state id -- ``"chat"``, ``"settings_sheet"``,
    ``"pairing"`` -- each a list of :class:`UITarget` (center + size in points).
    Conditionally-shown controls (e.g. the composer stop button) are included with
    ``exists=False`` so the cost model can price both states. Size assumptions are
    recorded in :data:`LAYOUT_ASSUMPTIONS` (also via :func:`assumptions`).
    """
    LAYOUT_ASSUMPTIONS.clear()
    root = source_root or _default_source_root()
    src = _read_sources(root)
    return {
        "chat": _chat_targets(src, screen_w_pt, screen_h_pt),
        "settings_sheet": _settings_targets(src, screen_w_pt, screen_h_pt),
        "pairing": _pairing_targets(src, screen_w_pt, screen_h_pt),
    }


def assumptions() -> dict[str, str]:
    """Return the assumption/provenance notes from the most recent build."""
    return dict(LAYOUT_ASSUMPTIONS)


# ---------------------------------------------------------------------------
# OPTIONAL: screenshot-based geometry (deterministic; needs NumPy + PIL).
# Clearly separated from the source-based map above.
# ---------------------------------------------------------------------------

# Theme.mint = 0x4DD9A6 -> the dominant accent for the send button (and checks).
_MINT_RGB: tuple[int, int, int] = (0x4D, 0xD9, 0xA6)


def _largest_color_component(mask) -> tuple[int, int, int, int] | None:
    """Bounding box (x0, y0, x1, y1) of the largest 4-connected True region.

    Pure-NumPy flood fill; deterministic. Returns None if the mask is empty.
    """
    import numpy as np  # local import: optional dependency

    visited = np.zeros_like(mask, dtype=bool)
    h, w = mask.shape
    best: tuple[int, tuple[int, int, int, int]] | None = None
    ys, xs = np.nonzero(mask)
    for sy, sx in zip(ys.tolist(), xs.tolist()):
        if visited[sy, sx]:
            continue
        # Iterative flood fill from this seed.
        stack = [(sy, sx)]
        visited[sy, sx] = True
        x0 = x1 = sx
        y0 = y1 = sy
        size = 0
        while stack:
            cy, cx = stack.pop()
            size += 1
            x0, x1 = min(x0, cx), max(x1, cx)
            y0, y1 = min(y0, cy), max(y1, cy)
            for ny, nx in ((cy - 1, cx), (cy + 1, cx), (cy, cx - 1), (cy, cx + 1)):
                if 0 <= ny < h and 0 <= nx < w and mask[ny, nx] and not visited[ny, nx]:
                    visited[ny, nx] = True
                    stack.append((ny, nx))
        if best is None or size > best[0]:
            best = (size, (x0, y0, x1, y1))
    return best[1] if best is not None else None


def build_ui_map_from_screenshot(
    png_path: str,
    scale: float = 3.0,
    color_tol: int = 40,
) -> dict[str, list[UITarget]]:
    """Measure control geometry from a real screenshot (deterministic).

    Detects the dominant ``Theme.mint`` blob (the send button's filled circle) by
    color thresholding and returns its measured center + size, converting pixels
    to points via ``/ scale`` (simulator screenshots are typically @3x). This is a
    pixel-measured complement to :func:`build_ui_map`; it returns ``{"chat":
    [UITarget("send_measured", ...)]}`` (empty if no mint blob is found).

    Requires NumPy + PIL. Use it on screenshots under ``/tmp/jcode-ui-matrix/``.
    """
    import numpy as np  # local imports: optional deps
    from PIL import Image

    arr = np.asarray(Image.open(png_path).convert("RGB"), dtype=np.int16)
    diff = np.abs(arr - np.array(_MINT_RGB, dtype=np.int16))
    mask = (diff.max(axis=2) <= color_tol)

    out: dict[str, list[UITarget]] = {"chat": []}
    box = _largest_color_component(mask)
    if box is not None:
        x0, y0, x1, y1 = box
        w_px = (x1 - x0 + 1)
        h_px = (y1 - y0 + 1)
        out["chat"].append(
            UITarget(
                id="send_measured",
                width_pt=w_px / scale,
                height_pt=h_px / scale,
                x_pt=(x0 + x1 + 1) / 2.0 / scale,
                y_pt=(y0 + y1 + 1) / 2.0 / scale,
                exists=True,
            )
        )
    return out


# ---------------------------------------------------------------------------
# CLI / self-test.
# ---------------------------------------------------------------------------

def _as_jsonable(m: dict[str, list[UITarget]]) -> dict:
    return {
        screen: [
            {
                "id": t.id,
                "width_pt": round(t.width_pt, 2),
                "height_pt": round(t.height_pt, 2),
                "x_pt": round(t.x_pt, 2),
                "y_pt": round(t.y_pt, 2),
                "exists": t.exists,
            }
            for t in targets
        ]
        for screen, targets in m.items()
    }


def _self_test() -> None:
    """Lightweight, deterministic invariants (source-based + synthetic screenshot)."""
    m = build_ui_map()
    chat = {t.id: t for t in m["chat"]}

    # The recently-set 44x44 send button must read 44x44, near the bottom-right.
    send = chat["send"]
    assert (send.width_pt, send.height_pt) == (44.0, 44.0), send
    assert send.x_pt > DEFAULT_SCREEN_W_PT * 0.75, "send should be on the right"
    assert send.y_pt > DEFAULT_SCREEN_H_PT * 0.80, "send should be near the bottom"

    # The 44x44 settings ellipsis must read 44x44, near the top-right.
    settings = chat["settings"]
    assert (settings.width_pt, settings.height_pt) == (44.0, 44.0), settings
    assert settings.x_pt > DEFAULT_SCREEN_W_PT * 0.75, "settings should be on the right"
    assert settings.y_pt < DEFAULT_SCREEN_H_PT * 0.20, "settings should be near the top"

    # Stop button is conditional -> mapped but exists=False.
    assert chat["stop"].exists is False

    # Screen-size parametrization shifts the bottom-pinned send button down.
    taller = build_ui_map(screen_h_pt=1000.0)
    send_tall = {t.id: t for t in taller["chat"]}["send"]
    assert send_tall.y_pt > send.y_pt, "taller screen pushes bottom controls down"

    # Synthetic screenshot round-trip for the optional measured variant.
    try:
        import numpy as np
        from PIL import Image
        import tempfile

        scale = 3.0
        W, H = int(DEFAULT_SCREEN_W_PT * scale), int(DEFAULT_SCREEN_H_PT * scale)
        img = np.full((H, W, 3), (0x0F, 0x0F, 0x14), dtype=np.uint8)  # Theme.background
        cx, cy, r = int(send.x_pt * scale), int(send.y_pt * scale), int(22 * scale)
        yy, xx = np.ogrid[:H, :W]
        circle = (xx - cx) ** 2 + (yy - cy) ** 2 <= r ** 2
        img[circle] = _MINT_RGB
        with tempfile.NamedTemporaryFile(suffix=".png", delete=True) as tf:
            Image.fromarray(img).save(tf.name)
            measured = build_ui_map_from_screenshot(tf.name, scale=scale)["chat"]
        assert measured, "expected a measured send blob"
        msend = measured[0]
        assert abs(msend.x_pt - send.x_pt) < 4 and abs(msend.y_pt - send.y_pt) < 4, msend
        assert abs(msend.width_pt - 44.0) < 4, msend
        print("screenshot self-test: OK (synthetic mint blob recovered)")
    except ImportError:
        print("screenshot self-test: skipped (NumPy/PIL not installed)")

    print("source self-test: OK")


if __name__ == "__main__":
    ui_map = build_ui_map()
    print(json.dumps(_as_jsonable(ui_map), indent=2))
    print("\n# layout assumptions / provenance:")
    print(json.dumps(assumptions(), indent=2))
    _self_test()
