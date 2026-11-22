#!/usr/bin/env python3
"""Unit tests for scripts/compose_before_after.py."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest

try:
    from PIL import Image
    HAS_PIL = True
except ImportError:
    HAS_PIL = False


if HAS_PIL:
    SCRIPT_PATH = Path(__file__).resolve().parent / "compose_before_after.py"
    SPEC = importlib.util.spec_from_file_location("compose_before_after", SCRIPT_PATH)
    assert SPEC and SPEC.loader
    MODULE = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(MODULE)


@unittest.skipUnless(HAS_PIL, "Pillow is not installed")
class ComposeBeforeAfterTests(unittest.TestCase):
    def test_compose_two_pngs_with_title_and_height_normalization(self) -> None:
        before = Image.new("RGB", (200, 100), (40, 20, 20))
        after = Image.new("RGB", (200, 120), (20, 40, 20))
        composed = MODULE.compose_before_after(
            before,
            after,
            before_label="Before (v1.0.3)",
            after_label="After (v1.0.4)",
            title="Header layout",
        )
        self.assertGreater(composed.width, 400)
        self.assertGreater(composed.height, 150)

    def test_cli_renders_ansi_text_frames_to_png(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before_txt = root / "before.txt"
            after_txt = root / "after.txt"
            out_png = root / "comparison.png"
            before_txt.write_text("\x1b[31mjcode v1.0.3\x1b[0m\nold header\n", encoding="utf-8")
            after_txt.write_text("\x1b[32mjcode v1.0.4\x1b[0m\nnew compact header\n", encoding="utf-8")
            rc = MODULE.main([
                "--before-text",
                str(before_txt),
                "--after-text",
                str(after_txt),
                "--before-label",
                "Before (v1.0.3)",
                "--after-label",
                "After (v1.0.4)",
                "--title",
                "TUI header",
                "--output",
                str(out_png),
            ])
            self.assertEqual(rc, 0)
            self.assertTrue(out_png.is_file())
            with Image.open(out_png) as img:
                self.assertEqual(img.format, "PNG")
                self.assertGreater(img.width, 300)
                self.assertGreater(img.height, 100)


if __name__ == "__main__":
    unittest.main()
