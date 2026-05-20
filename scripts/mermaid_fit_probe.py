#!/usr/bin/env python3
"""Probe Jcode pinned Mermaid fit planning for screenshot repros.

This intentionally mirrors the geometry math in src/tui/ui_diagram_pane.rs so a
bad crop can be reproduced without compiling the whole Rust test binary.
Defaults are the 2026-05-07 Beetle/Harbor clipped Mermaid screenshot:
  inner pane 73x46 cells, font 8x16 px, PNG 1180x1470 px.
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
from typing import Any

TARGET_UTILIZATION_PERCENT = 85.0
MIN_READABLE_ZOOM_PERCENT = 70
MAX_AUTO_FILL_ZOOM_PERCENT = 1000


def clamp(value: int, lo: int, hi: int) -> int:
    return max(lo, min(hi, value))


def utilization_percent(used: int, total: int) -> float:
    return 0.0 if total == 0 else (used * 100.0) / total


def div_ceil(value: int, divisor: int) -> int:
    return 0 if divisor == 0 else (value + divisor - 1) // divisor


def axis_fill_zoom_percent(available_cells: int, image_px: int, cell_px: int) -> int:
    if available_cells == 0 or image_px == 0 or cell_px == 0:
        return 100
    return clamp((available_cells * cell_px * 100) // max(image_px, 1), 1, MAX_AUTO_FILL_ZOOM_PERCENT)


def fit_zoom_percent_for_area(width_cells: int, height_cells: int, img_w_px: int, img_h_px: int, font_w: int, font_h: int) -> int:
    if width_cells == 0 or height_cells == 0 or img_w_px == 0 or img_h_px == 0:
        return 100
    zoom_w = (width_cells * max(font_w, 1) * 100) // max(img_w_px, 1)
    zoom_h = (height_cells * max(font_h, 1) * 100) // max(img_h_px, 1)
    return clamp(min(zoom_w, zoom_h), 1, MAX_AUTO_FILL_ZOOM_PERCENT)


def vcenter_fitted(width_cells: int, height_cells: int, img_w_px: int, img_h_px: int, font_w: int, font_h: int) -> dict[str, int]:
    if width_cells == 0 or height_cells == 0 or img_w_px == 0 or img_h_px == 0:
        return {"x": 0, "y": 0, "width": width_cells, "height": height_cells}
    area_w_px = width_cells * max(font_w, 1)
    area_h_px = height_cells * max(font_h, 1)
    scale = min(area_w_px / img_w_px, area_h_px / img_h_px)
    fitted_w = min(math.ceil((img_w_px * scale) / max(font_w, 1)), width_cells)
    fitted_h = min(math.ceil((img_h_px * scale) / max(font_h, 1)), height_cells)
    return {
        "x": (width_cells - fitted_w) // 2,
        "y": (height_cells - fitted_h) // 2,
        "width": fitted_w,
        "height": fitted_h,
    }


def centered_viewport_scroll_cells(image_px: int, area_cells: int, zoom_percent: int, cell_px: int) -> int:
    if image_px == 0 or area_cells == 0 or zoom_percent == 0 or cell_px == 0:
        return 0
    view_px = area_cells * cell_px * 100 // zoom_percent
    max_scroll_px = max(0, image_px - view_px)
    if max_scroll_px == 0:
        return 0
    cell_px_at_zoom = max(div_ceil(cell_px * 100, zoom_percent), 1)
    return (max_scroll_px // 2) // cell_px_at_zoom


def plan(width_cells: int, height_cells: int, img_w_px: int, img_h_px: int, font_w: int, font_h: int) -> dict[str, Any]:
    contain = vcenter_fitted(width_cells, height_cells, img_w_px, img_h_px, font_w, font_h)
    fit_zoom = fit_zoom_percent_for_area(width_cells, height_cells, img_w_px, img_h_px, font_w, font_h)
    width_fill_zoom = axis_fill_zoom_percent(width_cells, img_w_px, font_w)
    height_fill_zoom = axis_fill_zoom_percent(height_cells, img_h_px, font_h)
    preferred_fill_zoom = clamp(max(width_fill_zoom, height_fill_zoom), MIN_READABLE_ZOOM_PERCENT, MAX_AUTO_FILL_ZOOM_PERCENT)

    width_utilization = utilization_percent(contain["width"], width_cells)
    height_utilization = utilization_percent(contain["height"], height_cells)
    area_utilization = utilization_percent(contain["width"] * contain["height"], width_cells * height_cells)
    underutilized = (
        width_utilization < TARGET_UTILIZATION_PERCENT
        or height_utilization < TARGET_UTILIZATION_PERCENT
        or area_utilization < TARGET_UTILIZATION_PERCENT
    )
    meaningfully_larger = preferred_fill_zoom > fit_zoom + 5

    old_would_fill = (fit_zoom < MIN_READABLE_ZOOM_PERCENT or underutilized) and meaningfully_larger
    fixed_would_fill = underutilized and meaningfully_larger

    fill_plan = {
        "mode": f"fit-fill@{preferred_fill_zoom}%",
        "zoom_percent": preferred_fill_zoom,
        "scroll_x": centered_viewport_scroll_cells(img_w_px, width_cells, preferred_fill_zoom, font_w),
        "scroll_y": centered_viewport_scroll_cells(img_h_px, height_cells, preferred_fill_zoom, font_h),
    }

    return {
        "input": {
            "inner_width_cells": width_cells,
            "inner_height_cells": height_cells,
            "image_width_px": img_w_px,
            "image_height_px": img_h_px,
            "font_width_px": font_w,
            "font_height_px": font_h,
        },
        "contain_rect_cells": contain,
        "utilization_percent": {
            "width": width_utilization,
            "height": height_utilization,
            "area": area_utilization,
        },
        "fit_zoom_percent": fit_zoom,
        "axis_fill_zoom_percent": {"width": width_fill_zoom, "height": height_fill_zoom},
        "preferred_fill_zoom_percent": preferred_fill_zoom,
        "underutilized": underutilized,
        "meaningfully_larger": meaningfully_larger,
        "old_buggy_plan": fill_plan if old_would_fill else {"mode": "fit", "rect": contain},
        "fixed_plan": fill_plan if fixed_would_fill else {"mode": "fit", "rect": contain},
        "repro_was_clipping_bug": old_would_fill and not fixed_would_fill,
    }


def maybe_png_info(path: str | None) -> dict[str, Any] | None:
    if not path:
        return None
    png = Path(path).expanduser()
    info: dict[str, Any] = {"path": str(png), "exists": png.exists()}
    if not png.exists():
        return info
    try:
        from PIL import Image  # type: ignore
    except Exception as exc:  # pragma: no cover - diagnostic fallback
        info["pil_error"] = str(exc)
        return info
    im = Image.open(png).convert("RGBA")
    bbox = im.getchannel("A").getbbox()
    info["size"] = list(im.size)
    info["alpha_bbox"] = list(bbox) if bbox else None
    if bbox:
        left, top, right, bottom = bbox
        info["content_size"] = [right - left, bottom - top]
        info["transparent_margins"] = {
            "left": left,
            "top": top,
            "right": im.size[0] - right,
            "bottom": im.size[1] - bottom,
        }
    return info


def visual_fit_metrics(
    *,
    width_cells: int,
    height_cells: int,
    img_w_px: int,
    img_h_px: int,
    font_w: int,
    font_h: int,
    contain_rect: dict[str, int],
    alpha_bbox: list[int] | tuple[int, int, int, int] | None,
) -> dict[str, Any] | None:
    """Project the visible PNG alpha bbox into the TUI pane.

    The layout planner works in cells and the PNG alpha bbox works in pixels.
    Projecting the bbox into cells gives a camera-like metric that matches the
    visual complaint: even if the full image rect is centered, is the actual
    non-transparent diagram content centered in the pane?
    """
    if not alpha_bbox or img_w_px <= 0 or img_h_px <= 0 or font_w <= 0 or font_h <= 0:
        return None

    left_px, top_px, right_px, bottom_px = [float(value) for value in alpha_bbox]
    render_w_px = contain_rect["width"] * font_w
    render_h_px = contain_rect["height"] * font_h
    scale_x = render_w_px / img_w_px
    scale_y = render_h_px / img_h_px

    content_left = contain_rect["x"] + (left_px * scale_x / font_w)
    content_top = contain_rect["y"] + (top_px * scale_y / font_h)
    content_right = contain_rect["x"] + (right_px * scale_x / font_w)
    content_bottom = contain_rect["y"] + (bottom_px * scale_y / font_h)
    content_width = max(0.0, content_right - content_left)
    content_height = max(0.0, content_bottom - content_top)

    left_blank = content_left
    right_blank = width_cells - content_right
    top_blank = content_top
    bottom_blank = height_cells - content_bottom
    center_x = (content_left + content_right) / 2.0
    center_y = (content_top + content_bottom) / 2.0
    pane_center_x = width_cells / 2.0
    pane_center_y = height_cells / 2.0
    offset_x = center_x - pane_center_x
    offset_y = center_y - pane_center_y

    return {
        "content_bbox_cells": {
            "left": content_left,
            "top": content_top,
            "right": content_right,
            "bottom": content_bottom,
            "width": content_width,
            "height": content_height,
        },
        "blank_cells": {
            "left": left_blank,
            "right": right_blank,
            "top": top_blank,
            "bottom": bottom_blank,
        },
        "blank_imbalance_cells": {
            "horizontal_right_minus_left": right_blank - left_blank,
            "vertical_bottom_minus_top": bottom_blank - top_blank,
        },
        "content_center_offset_cells": {
            "x": offset_x,
            "y": offset_y,
        },
        "content_area_utilization_percent": utilization_percent(
            int(round(content_width * content_height * 1000)),
            int(width_cells * height_cells * 1000),
        ),
        "camera_centered": abs(offset_x) <= 1.0 and abs(offset_y) <= 1.0,
        "note": (
            "Low content_area_utilization can be normal for a wide/short or tall/narrow diagram. "
            "A large content_center_offset or top/bottom blank imbalance means the camera/placement is wrong."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inner", default="73x46", help="inner pane size in cells, WIDTHxHEIGHT")
    parser.add_argument("--image", default="1180x1470", help="rendered PNG size in px, WIDTHxHEIGHT")
    parser.add_argument("--font", default="8x16", help="terminal cell size in px, WIDTHxHEIGHT")
    parser.add_argument("--png", help="optional rendered PNG path to inspect alpha bounds")
    parser.add_argument(
        "--alpha-bbox",
        help="optional alpha/content bbox LEFT,TOP,RIGHT,BOTTOM in PNG pixels when --png is unavailable",
    )
    parser.add_argument(
        "--assert-camera-centered",
        action="store_true",
        help="fail if projected visible content is more than one cell away from pane center",
    )
    parser.add_argument(
        "--assert-old-clipping-repro",
        action="store_true",
        help="fail unless the old readability-floor rule would have produced the known clipping bug",
    )
    args = parser.parse_args()

    def parse_pair(raw: str) -> tuple[int, int]:
        left, right = raw.lower().split("x", 1)
        return int(left), int(right)

    width_cells, height_cells = parse_pair(args.inner)
    img_w_px, img_h_px = parse_pair(args.image)
    font_w, font_h = parse_pair(args.font)
    result = plan(width_cells, height_cells, img_w_px, img_h_px, font_w, font_h)
    png_info = maybe_png_info(args.png)
    if png_info is not None:
        result["png"] = png_info
    alpha_bbox = None
    if args.alpha_bbox:
        alpha_bbox = [int(part) for part in args.alpha_bbox.split(",")]
    elif png_info is not None:
        alpha_bbox = png_info.get("alpha_bbox")
    metrics = visual_fit_metrics(
        width_cells=width_cells,
        height_cells=height_cells,
        img_w_px=img_w_px,
        img_h_px=img_h_px,
        font_w=font_w,
        font_h=font_h,
        contain_rect=result["contain_rect_cells"],
        alpha_bbox=alpha_bbox,
    )
    if metrics is not None:
        result["visual_fit_metrics"] = metrics
    print(json.dumps(result, indent=2, sort_keys=True))
    if args.assert_camera_centered and metrics is not None:
        return 0 if metrics["camera_centered"] else 2
    if args.assert_old_clipping_repro:
        return 0 if result["repro_was_clipping_bug"] else 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
