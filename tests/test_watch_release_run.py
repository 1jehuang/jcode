#!/usr/bin/env python3
"""Run the release watcher against a fake gh CLI, without network or delays."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/watch_release_run.sh"


class WatchReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        gh = self.directory / "gh"
        gh.write_text('#!/bin/sh\ncase "$*" in *status,conclusion*) printf "%s\\n" "$TEST_RUN_STATUS" ;; *) printf "job diagnostics\\n" ;; esac\n')
        gh.chmod(0o755)
        self.env = dict(os.environ, PATH=f"{self.directory}:{os.environ['PATH']}")

    def watch(self, status):
        return subprocess.run(["bash", str(SCRIPT), "123", "1", "0"],
                              env=dict(self.env, TEST_RUN_STATUS=status),
                              capture_output=True, text=True, timeout=5)

    def test_success(self):
        result = self.watch("completed success")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("RUN SUCCESS", result.stdout)

    def test_all_non_success_terminal_conclusions_fail_immediately(self):
        for conclusion in ("failure", "cancelled", "timed_out", "action_required",
                           "startup_failure", "stale", "skipped", "neutral"):
            with self.subTest(conclusion=conclusion):
                result = self.watch(f"completed {conclusion}")
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn("RUN FAILED", result.stdout)
                self.assertIn("job diagnostics", result.stdout)

    def test_pending_does_not_claim_success_or_failure(self):
        result = self.watch("in_progress ")
        self.assertEqual(result.returncode, 2)
        self.assertIn("RUN STILL RUNNING", result.stdout)

    def test_default_budget_covers_release_job_timeouts(self):
        script = SCRIPT.read_text()
        self.assertIn('MAX_POLLS="${2:-480}"', script)
        self.assertIn('SLEEP_S="${3:-30}"', script)
        # Four hours exceeds the 150-minute Windows build plus finalization,
        # and the actual previous successful 71-minute v0.97 recovery run.
        self.assertGreater(480 * 30 / 60, 150 + 15)


if __name__ == "__main__":
    unittest.main()
