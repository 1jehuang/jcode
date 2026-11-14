#!/usr/bin/env python3
"""Pure tests run with Python. Native tests run with Kitty's embedded Python:
kitty +runpy 'import runpy; runpy.run_path("scripts/test_render_kitty_frames.py", run_name="__main__")'
"""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from render_kitty_frames import KittyRasterizer, frame_repeats, linear_to_byte, srgb_to_linear

try:
    import kitty.fast_data_types
    NATIVE = True
except ImportError:
    NATIVE = False


class MathTests(unittest.TestCase):
    def test_color_round_trip(self):
        for x in range(256):
            self.assertEqual(x, linear_to_byte(srgb_to_linear(x / 255)))

    def test_timestamps(self):
        self.assertEqual([3, 1], frame_repeats([{"t_ms":0}, {"t_ms":100}], 24))
        self.assertEqual([24, 24], frame_repeats([{"t_ms":0}, {"t_ms":1000}], 24, 2000))
        self.assertEqual([], frame_repeats([], 24))


@unittest.skipUnless(NATIVE, "run via kitty +runpy for native glyph tests")
class NativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="kitty-render-", dir=__import__("os").environ.get("JCODE_SCRATCH_DIR"))
        cls.config = Path(cls.tmp.name) / "kitty.conf"
        cls.config.write_text("font_family monospace\nfont_size 11\nforeground #cdd6f4\nbackground #000000\ntext_composition_strategy 1.5 70\n")
        cls.renderer = KittyRasterizer(12, 3, cls.config)

    @classmethod
    def tearDownClass(cls):
        cls.renderer.close()
        cls.tmp.cleanup()

    def image(self, ansi):
        r = self.renderer
        return r.Image.frombytes("RGB", (12 * 14, 3 * 32), r.render(ansi))

    def test_native_dimensions_and_background(self):
        image = self.image(b"")
        self.assertEqual(image.size, (168, 96))
        self.assertEqual(image.getextrema(), ((0, 0), (0, 0), (0, 0)))

    def test_rgb_background_and_reset(self):
        image = self.image(b"\x1b[48;2;12;34;56m \x1b[0m")
        self.assertEqual(image.getpixel((5, 5)), (12, 34, 56))
        self.assertEqual(image.getpixel((20, 5)), (0, 0, 0))
        self.assertEqual(self.image(b"").getpixel((5, 5)), (0, 0, 0))

    def test_bold_italic_wide_emoji_and_underline(self):
        regular = self.image(b"Hello").tobytes()
        self.assertNotEqual(regular, self.image(b"\x1b[1mHello").tobytes())
        self.assertNotEqual(regular, self.image(b"\x1b[3mHello").tobytes())
        styled = self.image("\x1b[4mhi\x1b[0m 😺界".encode())
        self.assertNotEqual(styled.getbbox(), None)
        r = self.renderer
        self.assertEqual(r.screen.line(0).width(3), 2)
        self.assertEqual(r.screen.line(0).width(5), 2)

    def test_native_packed_alpha_not_rgba_byte_order(self):
        from kitty.fast_data_types import sprite_idx_to_pos
        image = self.image(b"A")
        r = self.renderer
        sprite = r.screen.line(0).sprite_at(0)
        raw = r.sprites[sprite_idx_to_pos(sprite, r.xnum, r.ynum)]
        zero_count = full_count = 0
        for pixel in range(14 * 32):
            word = int.from_bytes(raw[pixel * 4:pixel * 4 + 4], sys.byteorder)
            alpha = word & 255
            actual = image.getpixel((pixel % 14, pixel // 14))
            if alpha == 0:
                self.assertEqual(actual, (0, 0, 0))
                zero_count += 1
            elif alpha == 255:
                self.assertEqual(actual, (205, 214, 244))
                full_count += 1
        self.assertGreater(zero_count, 100)
        self.assertGreater(full_count, 10)

    def test_long_ansi_chunking_and_reproducibility(self):
        data = b"\x1b[0m" * 5000 + b"hello"
        self.assertEqual(self.image(data).tobytes(), self.image(b"hello").tobytes())

    @unittest.skipUnless(shutil.which("ffmpeg") and shutil.which("ffprobe"), "ffmpeg required")
    def test_cli_pipes_rgb_directly_to_video(self):
        directory = Path(self.tmp.name)
        (directory / "frame.ansi").write_bytes(b"\x1b[38;2;20;200;100mKitty native")
        manifest = directory / "manifest.json"
        manifest.write_text(json.dumps({"cols":12,"rows":3,"fps":24,"frames":[{"t_ms":0,"ansi":"frame.ansi"}]}))
        output = directory / "smoke.mp4"
        subprocess.run(["python3", str(HERE / "render_kitty_frames.py"), str(manifest), "--config", str(self.config), "-o", str(output)], check=True)
        result = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "stream=width,height,nb_frames,pix_fmt", "-of", "json", str(output)], capture_output=True, text=True, check=True)
        stream = json.loads(result.stdout)["streams"][0]
        self.assertEqual((stream["width"], stream["height"]), (168,96))
        self.assertEqual(stream["nb_frames"], "1")
        self.assertEqual(stream["pix_fmt"], "yuv444p")
        lossless = directory / "smoke.mkv"
        subprocess.run(["python3", str(HERE / "render_kitty_frames.py"), str(manifest), "--config", str(self.config), "--codec", "ffv1", "-o", str(lossless)], check=True)
        decoded = subprocess.run(["ffmpeg", "-v", "error", "-i", str(lossless), "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"], capture_output=True, check=True).stdout
        self.assertEqual(decoded, self.renderer.render((directory / "frame.ansi").read_bytes()))


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]])
