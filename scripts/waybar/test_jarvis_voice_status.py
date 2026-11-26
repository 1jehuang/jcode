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
    def test_trigger_is_visible_without_disclosing_caption(self):
        value = {"capturing": True, "state": "triggered", "caption": "private phrase"}
        for renderer in (module.render, module.native_render):
            result = renderer(value)
            self.assertEqual(result["class"], "triggered")
            self.assertIn("Voice request accepted", result["tooltip"])
            self.assertNotIn("private phrase", json.dumps(result))

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

    def test_native_stream_is_measured_bounded_and_finite(self):
        result = module.native_render({"pid": os.getpid(), "capturing": True,
                                      "state": "listening", "levels": [-1, .25, float("nan"), 3],
                                      "sequence": 7, "sample_interval_ms": 50})
        self.assertEqual(result["levels"], [0, .25, 0, 1])
        self.assertEqual(result["sequence"], 7)
        self.assertEqual(result["sample_interval_ms"], 50)
        self.assertEqual(result["text"], "")
        self.assertNotIn("transcript", result)
        json.dumps(result, allow_nan=False)
        result = module.native_render({"capturing": True, "levels": [.5] * 1000})
        self.assertEqual(len(result["levels"]), 128)
        self.assertEqual(result["sample_interval_ms"], 200)

    def test_native_pause_and_off_do_not_claim_capture(self):
        self.assertFalse(module.native_render(None)["capturing"])
        result = module.native_render({"capturing": False, "state": "paused", "levels": []})
        self.assertEqual(result["class"], "paused")
        self.assertFalse(result["capturing"])

    def test_error_click_is_a_private_single_request_never_a_pause_toggle(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / f"{os.getpid()}.json"
            value = {"pid": os.getpid(), "capturing": False, "state": "error",
                     "error_reason": "Nari credits are exhausted", "error_hint": "Add credits.",
                     "retry_supported": True, "retry_token": "0123456789abcdef", "transcript": "private"}
            path.write_text(json.dumps(value))
            for renderer in (module.render, module.native_render):
                result = renderer(value)
                self.assertEqual(result["class"], "error")
                self.assertIn("Nari credits", result["tooltip"])
                self.assertIn("Add credits", result["tooltip"])
                self.assertIn("click to request", result["tooltip"])
                self.assertNotIn("private", json.dumps(result))
            self.assertEqual(module.native_render(value)["state"], "error")
            self.assertIsNone(module.toggle(root))
            self.assertIsNone(module.toggle(root))
            self.assertEqual(path.with_suffix(".retry-0123456789abcdef").stat().st_mode & 0o777, 0o600)
            self.assertFalse(path.with_suffix(".paused").exists())

    def test_legacy_or_malformed_error_does_not_imply_retry_or_create_any_file(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / f"{os.getpid()}.json"
            for capability in [{}, {"retry_supported": "true"}, {"retry_supported": True, "retry_token": "../oops"}]:
                value = {"pid": os.getpid(), "capturing": False, "state": "error", **capability}
                path.write_text(json.dumps(value))
                self.assertIn("Open the Jarvis client", module.toggle(root))
                self.assertIn("does not support Waybar retry", module.native_render(value)["tooltip"])
                self.assertEqual(list(root.iterdir()), [path])

    def test_native_metadata_is_clamped(self):
        result = module.native_render({"capturing": True, "state": "x" * 1000,
                                      "sequence": -4, "sample_interval_ms": 9000,
                                      "levels": "not samples"})
        self.assertEqual(len(result["state"]), 80)
        self.assertEqual(result["sequence"], 0)
        self.assertEqual(result["sample_interval_ms"], 1000)
        self.assertEqual(result["levels"], [])

    def test_malformed_status_is_ignored(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / f"{os.getpid()}.json"
            for value in ["{", "[]", "null", '{"pid":0,"capturing":true}']:
                path.write_text(value)
                self.assertIsNone(module.latest_status(root))


if __name__ == "__main__":
    unittest.main()
