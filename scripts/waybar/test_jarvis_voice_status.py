import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location("jarvis_status", Path(__file__).with_name("jarvis-voice-status.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class StatusTests(unittest.TestCase):
    def test_off_does_not_claim_capture(self):
        self.assertEqual(module.render(None)["text"], "")
        self.assertEqual(module.render({"capturing": False, "state": "paused"})["text"], "🎙 paused")

    def test_waveform_uses_actual_levels_and_is_fixed_width(self):
        result = module.render({"capturing": True, "state": "listening", "levels": [0, .5, 1, float("nan")]})
        self.assertTrue(result["text"].startswith("🎙 ▁▅█▁"))
        self.assertEqual(len(result["text"].split(" ", 1)[1]), 12)

    def test_status_requires_fresh_owned_live_process(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / f"{os.getpid()}.json"
            path.write_text(json.dumps({"pid": os.getpid(), "capturing": True}))
            self.assertIsNotNone(module.latest_status(root))
            self.assertIsNone(module.latest_status(root, time.time() + 10))
            root.chmod(0o777)
            self.assertIsNone(module.latest_status(root))

    def test_toggle_is_private_and_reversible(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / f"{os.getpid()}.json"
            path.write_text(json.dumps({"pid": os.getpid(), "capturing": True}))
            module.toggle(root)
            self.assertEqual(path.with_suffix(".paused").stat().st_mode & 0o777, 0o600)
            module.toggle(root)
            self.assertFalse(path.with_suffix(".paused").exists())

    def test_malformed_status_is_ignored(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / f"{os.getpid()}.json"
            for value in ["{", "[]", "null", '{"pid":0,"capturing":true}']:
                path.write_text(value)
                self.assertIsNone(module.latest_status(root))


if __name__ == "__main__":
    unittest.main()
