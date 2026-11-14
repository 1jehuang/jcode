#!/usr/bin/env python3
"""Headless Kitty-native ANSI frame rasterizer. No display server or screenshots.

Run: kitty +runpy 'import runpy; runpy.run_path("scripts/render_kitty_frames.py", run_name="__main__")' -- manifest.json -o video.mp4
Or simply: python3 scripts/render_kitty_frames.py manifest.json -o video.mp4
The latter re-execs Kitty's embedded Python to load its native extension.

Manifest: {"cols":120,"rows":30,"fps":24,"frames":[{"t_ms":0,
"ansi":"frame-00000.ansi"}]}. ANSI files are independent full terminal frames.
--raw writes consecutive packed rgb24 frames directly instead of starting ffmpeg.
Background is deliberately opaque: desktop transparency/blur cannot be represented
by terminal cells alone. Cursor is hidden. Glyphs, shaping, fallback, bold, italic,
wide characters and emoji are produced by Kitty itself, never approximated.
"""
from __future__ import annotations

import argparse
from collections import OrderedDict
import json
import math
import os
from pathlib import Path
import subprocess
import sys


def srgb_to_linear(x):
    return x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4


def linear_to_byte(x):
    x = max(0.0, min(1.0, x))
    return round(255 * (12.92 * x if x <= 0.0031308 else 1.055 * x ** (1 / 2.4) - 0.055))


def frame_repeats(frames, fps, duration_ms=None):
    """Quantize timestamps to fixed-rate video, preserving total duration."""
    if not frames:
        return []
    end = duration_ms if duration_ms is not None else frames[-1].get("t_ms", 0) + 1000 / fps
    total = max(1, math.ceil(end * fps / 1000 - 1e-8))
    counts = [0] * len(frames)
    index = 0
    for tick in range(total):
        while index + 1 < len(frames) and frames[index + 1].get("t_ms", 0) <= tick * 1000 / fps + 1e-6:
            index += 1
        counts[index] += 1
    return counts


class KittyRasterizer:
    """Native Kitty screen/parser and sprite atlas, with CPU shader composition."""
    def __init__(self, cols=120, rows=30, config=None, dpi=153.6,
                 font_size=None, cell_width=14, cell_height=32):
        from kitty.config import load_config
        from kitty.fast_data_types import (Screen, create_test_font_group,
            set_options, set_send_sprite_to_gpu, sprite_map_set_limits)
        from kitty.fonts.render import set_font_family
        from PIL import Image
        self.Image = Image
        self.opts = load_config(str(config)) if config else load_config()
        if font_size is not None:
            self.opts = self.opts._replace(font_size=font_size)
        set_options(self.opts)
        self.sprites = {}
        self.xnum, self.ynum = 100000, 100
        sprite_map_set_limits(self.xnum, self.ynum)
        set_send_sprite_to_gpu(lambda x, y, z, data: self.sprites.__setitem__((x, y, z), bytes(data)))
        set_font_family(self.opts, add_builtin_nerd_font=True)
        self.cw, self.ch, self.baseline = create_test_font_group(self.opts.font_size, dpi, dpi)
        if (self.cw, self.ch) != (cell_width, cell_height):
            self.close()
            raise ValueError(f"Native Kitty geometry {self.cw}x{self.ch}, requested {cell_width}x{cell_height}. Adjust --dpi/--font-size, not image scaling.")
        self.cols, self.rows = cols, rows
        self.screen = Screen(None, rows, cols)
        self.screen.color_profile.reload_from_opts(self.opts)
        strategy = self.opts.text_composition_strategy.split()
        self.legacy = strategy == ["legacy"]
        self.gamma = 1 / float(strategy[0]) if strategy and strategy[0] not in ("platform", "legacy") else 1.0
        self.contrast = 1 + float(strategy[1]) / 100 if len(strategy) > 1 else 1.0
        self.cache = OrderedDict()
        self.linear = [srgb_to_linear(v / 255) for v in range(256)]

    def close(self):
        from kitty.fast_data_types import set_send_sprite_to_gpu
        set_send_sprite_to_gpu(None)

    def parse(self, data):
        # Kitty's actual VT parser. Chunking respects its bounded native buffer.
        for start in range(0, len(data), 16384):
            buffer = self.screen.test_create_write_buffer()
            self.screen.test_commit_write_buffer(data[start:start + 16384], buffer)
            self.screen.test_parse_written_data()

    def color(self, packed, default):
        color = self.screen.color_profile.as_color(packed) if packed else default
        return (color.red, color.green, color.blue)

    def tile(self, sprite, cursor):
        from kitty.fast_data_types import sprite_idx_to_pos
        cp = self.screen.color_profile
        fg = self.color(cursor.fg, cp.default_fg)
        bg = self.color(cursor.bg, cp.default_bg)
        if cursor.reverse:
            fg, bg = bg, fg
        key = (sprite, fg, bg, cursor.dim, cursor.decoration, cursor.decoration_fg, cursor.strikethrough)
        if key in self.cache:
            self.cache.move_to_end(key)
            return self.cache[key]
        raw = self.sprites[sprite_idx_to_pos(sprite & 0x7fffffff, self.xnum, self.ynum)]
        colored = bool(sprite & 0x80000000)
        bl = tuple(self.linear[v] for v in bg)
        lum = lambda c: sum(a * b for a, b in zip(c, (0.2126, 0.7152, 0.0722)))
        dim = self.opts.dim_opacity if cursor.dim else 1.0
        # Cache a per-alpha lookup for monochrome glyphs. Kitty's fragment shader
        # blends in linear light, with luminance-dependent gamma/contrast.
        def blend(rgb, alpha):
            f = tuple(self.linear[v] for v in rgb)
            a = alpha / 255
            if self.legacy:
                lo, lu = lum(f), lum(bl)
                if abs(lo - lu) > 1e-10:
                    mixed = (linear_to_byte(lo) * a + linear_to_byte(lu) * (1 - a)) / 255
                    a = max(0, min(1, (srgb_to_linear(mixed) - lu) / (lo - lu)))
            else:
                mix = (1 - lum(f) + lum(bl)) / 2
                a = min(1, max(0, (a * (1 - mix) + a ** self.gamma * mix) * self.contrast))
            a *= dim
            return bytes(linear_to_byte(x * a + y * (1 - a)) for x, y in zip(f, bl))
        table = [blend(fg, a) for a in range(256)]
        pixels = bytearray()
        for i in range(0, self.cw * self.ch * 4, 4):
            # Native sprites are uint32 0xRRGGBBAA, in host byte order.
            # On little-endian hosts their bytes are A,B,G,R, not RGBA.
            word = int.from_bytes(raw[i:i + 4], sys.byteorder)
            alpha = word & 255
            rgb = ((word >> 24) & 255, (word >> 16) & 255, (word >> 8) & 255)
            pixels.extend(blend(rgb, alpha) if colored else table[alpha])
        tile = self.Image.frombytes("RGB", (self.cw, self.ch), bytes(pixels))
        if cursor.decoration or cursor.strikethrough:
            from kitty.fast_data_types import render_decoration, current_fonts
            face = current_fonts()["medium"]
            decfg = self.color(cursor.decoration_fg, cp.default_fg) if cursor.decoration_fg else fg
            kinds = []
            if cursor.decoration:
                kinds.append({1:"straight", 2:"double", 3:"curl", 4:"dotted", 5:"dashed"}.get(cursor.decoration, "straight"))
            if cursor.strikethrough:
                kinds.append("strikethrough")
            for kind in kinds:
                decoration = bytes(render_decoration(kind, self.cw, self.ch, face.underline_position, face.underline_thickness))
                mask = self.Image.frombytes("L", (self.cw, self.ch), decoration)
                tile.paste(decfg, (0, 0), mask)
        self.cache[key] = tile
        if len(self.cache) > 16384:
            self.cache.popitem(last=False)
        return tile

    def render(self, ansi):
        from kitty.fast_data_types import test_render_line
        self.parse(b"\x1bc\x1b[?25l" + ansi)
        image = self.Image.new("RGB", (self.cols * self.cw, self.rows * self.ch))
        for y in range(self.rows):
            line = self.screen.line(y)
            test_render_line(line)
            for x in range(self.cols):
                image.paste(self.tile(line.sprite_at(x), line.cursor_from(x)), (x * self.cw, y * self.ch))
        return image.tobytes()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("-o", "--output", default="-", help="video output, or RGB output with --raw")
    parser.add_argument("--raw", action="store_true")
    parser.add_argument("--config", type=Path, default=Path.home()/".config/kitty/kitty.conf")
    parser.add_argument("--dpi", type=float, default=153.6)
    parser.add_argument("--font-size", type=float)
    parser.add_argument("--cell-width", type=int, default=14)
    parser.add_argument("--cell-height", type=int, default=32)
    parser.add_argument("--fps", type=float)
    parser.add_argument("--limit", type=int, help="limit source frames for smoke tests")
    parser.add_argument("--crf", type=int, default=16)
    parser.add_argument("--codec", choices=("h264", "ffv1"), default="h264", help="ffv1 writes a lossless RGB master (use .mkv)")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    frames = manifest["frames"][:args.limit] if args.limit else manifest["frames"]
    if not frames:
        parser.error("manifest has no frames")
    cols, rows = manifest.get("cols",120), manifest.get("rows",30)
    fps = args.fps or manifest.get("fps",24)
    renderer = KittyRasterizer(cols, rows, args.config, args.dpi, args.font_size, args.cell_width, args.cell_height)
    width, height = cols * renderer.cw, rows * renderer.ch
    print(f"Kitty native {renderer.cw}x{renderer.ch} cells, {width}x{height} RGB, {fps:g} fps", file=sys.stderr)
    process = None
    stream = None
    try:
        if args.raw:
            stream = sys.stdout.buffer if args.output == "-" else open(args.output, "wb")
        else:
            command = ["ffmpeg", "-y", "-loglevel", "warning", "-f", "rawvideo", "-pixel_format", "rgb24", "-video_size", f"{width}x{height}", "-framerate", str(fps), "-i", "pipe:0", "-an"]
            if args.codec == "ffv1":
                command += ["-c:v", "ffv1", "-level", "3", "-pix_fmt", "bgr0"]
            else:
                command += ["-c:v", "libx264", "-preset", "fast", "-crf", str(args.crf), "-pix_fmt", "yuv444p", "-movflags", "+faststart"]
            command.append(args.output)
            process = subprocess.Popen(command, stdin=subprocess.PIPE)
            stream = process.stdin
        repeats = frame_repeats(frames, fps, None if args.limit else manifest.get("duration_ms"))
        previous_ansi_path = None
        rgb = None
        for index, (frame, count) in enumerate(zip(frames, repeats)):
            if not count:
                continue
            ansi_path = args.manifest.parent / frame["ansi"]
            if ansi_path != previous_ansi_path:
                rgb = renderer.render(ansi_path.read_bytes())
                previous_ansi_path = ansi_path
            for _ in range(count):
                stream.write(rgb)
            if index % 24 == 0:
                print(f"rendered source frame {index + 1}/{len(frames)}", file=sys.stderr)
    finally:
        renderer.close()
        if stream is not None and stream is not sys.stdout.buffer:
            stream.close()
        if process is not None and process.wait():
            raise RuntimeError("ffmpeg failed")


if __name__ == "__main__":
    try:
        import kitty.fast_data_types  # noqa: F401
    except ImportError:
        # +runpy executes code, not a path. No Kitty window is created.
        code = "import runpy,sys; sys.argv=" + repr([str(Path(__file__).resolve())] + sys.argv[1:]) + "; runpy.run_path(sys.argv[0], run_name='__main__')"
        os.execvp("kitty", ["kitty", "+runpy", code])
    main()
