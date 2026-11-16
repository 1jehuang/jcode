#!/usr/bin/env python3
"""Safety regression tests. No ffmpeg, builds, credentials, or desktop required."""
import argparse
import contextlib
import copy
import importlib.util
import io
import json
import math
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("launch_video", Path(__file__).with_name("launch_video.py"))
video = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(video)


class LaunchVideoTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.manifest = self.root / "edit.json"
        self.output = self.root / "out.mp4"
        self.data = {"version": 1, "intro": {"duration": 4}, "outro": {"duration": 4},
                     "clips": [{"path": "capture.mp4", "section": section, "duration": duration,
                                "caption": "Real work", "reviewed": True}
                               for section, duration in (("swarm", 20), ("cloud", 12), ("performance", 10))]}
        self.args = argparse.Namespace(manifest=str(self.manifest), output=str(self.output),
                                       overwrite=False, preset="ultrafast")

    def compose(self, data=None):
        self.manifest.write_text(json.dumps(self.data if data is None else data))
        with mock.patch.object(video.shutil, "which", return_value="/mock/bin/tool"):
            video.compose(self.args)

    def assert_rejected(self, pattern, data=None):
        with self.assertRaisesRegex(ValueError, pattern):
            self.compose(data)
        self.assertFalse(self.output.exists())

    def test_requires_version(self):
        self.data["version"] = 2
        self.assert_rejected("version")

    def test_privacy_review_is_explicit_boolean(self):
        for value in (False, None, 1, "true"):
            with self.subTest(value=value):
                self.data["clips"][0]["reviewed"] = value
                self.assert_rejected("reviewed:true")

    def test_missing_review_is_rejected(self):
        del self.data["clips"][0]["reviewed"]
        self.assert_rejected("reviewed:true")

    def test_duration_limits_and_invalid_numbers(self):
        for value in (0, -1, 100, math.nan, math.inf, True):
            with self.subTest(value=value):
                data = copy.deepcopy(self.data)
                data["clips"][0]["duration"] = value
                self.assert_rejected("duration|45-70", data)

    def test_start_and_speed_validation(self):
        for key, value in (("start", -1), ("start", math.nan), ("speed", 0), ("speed", math.inf)):
            with self.subTest(key=key, value=value):
                data = copy.deepcopy(self.data)
                data["clips"][0][key] = value
                self.assert_rejected(key, data)

    def test_required_sections_and_order(self):
        self.data["clips"][0]["section"] = "performance"
        self.assert_rejected("must include")
        self.data["clips"][0]["section"] = "swarm"
        self.data["clips"].reverse()
        self.assert_rejected("ordered")

    def test_no_clobber(self):
        self.output.write_bytes(b"existing delivery")
        with self.assertRaisesRegex(ValueError, "Output exists"):
            self.compose()
        self.assertEqual(self.output.read_bytes(), b"existing delivery")

    def reject_at_clip(self, pattern, clip_info=None):
        font = self.root / "font.ttf"
        font.write_bytes(b"font fixture")
        with mock.patch.object(video, "font_path", return_value=font), \
             mock.patch.object(video.subprocess, "run") as ffmpeg, \
             mock.patch.object(video, "probe", side_effect=[{"format": {"duration": "4"}}, clip_info]):
            self.assert_rejected(pattern)
            commands = [call.args[0] for call in ffmpeg.call_args_list]
            self.assertEqual(len(commands), 1, "Invalid clip must never be sent to ffmpeg")

    def test_missing_relative_path(self):
        self.reject_at_clip("Missing clip")

    def test_directory_is_not_video(self):
        (self.root / "capture.mp4").mkdir()
        self.reject_at_clip("Missing clip")

    def test_non_video_is_rejected(self):
        (self.root / "capture.mp4").touch()
        self.reject_at_clip("No video stream", {"streams": [{"codec_type": "audio"}]})

    def test_oversized_and_odd_dimensions(self):
        (self.root / "capture.mp4").touch()
        for width, height, pattern in ((1922, 960, "safe area"), (1680, 962, "safe area"),
                                       (1679, 960, "even"), (1680, 959, "even")):
            with self.subTest(width=width, height=height):
                self.reject_at_clip(pattern, {"streams": [{"codec_type": "video", "width": width, "height": height}]})

    def test_literal_text_uses_file_not_filter_interpolation(self):
        text = "100% ready: 'quoted', [not an expression]"
        expression = video.text_filter(self.root, text, 30, 1000, "caption")
        self.assertNotIn(text, expression)
        self.assertIn("expansion=none", expression)
        self.assertEqual((self.root / "textcaption.txt").read_text(), text)
        for invalid in ("bad\ncaption", "x" * 91):
            with self.assertRaises(ValueError):
                video.text_filter(self.root, invalid, 30, 1000, "bad")

    def test_sources_only_emit_metadata_not_transcripts(self):
        sentinel = "PRIVATE_TRANSCRIPT_SENTINEL_DO_NOT_COPY"
        session = {"id": "session_test", "title": "Public example", "messages": [
            {"content": [{"type": "text", "text": sentinel},
                         {"type": "tool_result", "content": sentinel},
                         {"type": "tool_use", "name": "todo", "input": {"phases": [
                             {"lanes": [{"lane": "one", "items": [{"description": sentinel}]},
                                        {"lane": "two"}, {"lane": "three"}]}]}},
                         {"type": "tool_use", "name": "swarm", "input": {"action": "spawn", "prompt": sentinel}}]}],
                   "replay_events": [{"event": "swarm_plan", "timestamp": "2026-01-01T00:00:00Z", "items": [sentinel]}]}
        (self.root / "session_test.json").write_text(json.dumps(session))
        (self.root / "session_test.journal.jsonl").write_text(json.dumps({
            "append_replay_events": [{"event": "swarm_status", "members": [sentinel]}]}) + "\n{partial")
        output = self.root / "sources.json"
        with contextlib.redirect_stdout(io.StringIO()):
            video.inspect_sources(argparse.Namespace(sessions=str(self.root), output=str(output), limit=20))
        raw = output.read_text()
        self.assertNotIn(sentinel, raw)
        self.assertNotIn("messages", json.loads(raw))
        inventory = json.loads(raw)
        self.assertEqual(inventory["candidates"][0]["event_counts"], {"swarm_plan": 1, "swarm_status": 1})
        self.assertEqual(inventory["tool_plan_candidates"][0]["event_counts"]["swarm_spawns"], 1)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_source_directory_and_limit_validation(self):
        for path, limit, pattern in ((self.root / "missing", 20, "does not exist"), (self.root, 0, "positive")):
            with self.assertRaisesRegex(ValueError, pattern):
                video.inspect_sources(argparse.Namespace(sessions=str(path), output=str(self.root / "sources.json"), limit=limit))


if __name__ == "__main__":
    unittest.main()
