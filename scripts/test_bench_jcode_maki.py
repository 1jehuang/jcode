#!/usr/bin/env python3
"""Tests for the Linux-only, no-model-call Jcode/Maki benchmark harness."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from bench_jcode_maki import Terminal, echo, snapshot, stop


class BenchmarkTests(unittest.TestCase):
    def test_snapshot_includes_live_process(self):
        result = snapshot([os.getpid()])
        self.assertGreaterEqual(result["processes"], 1)
        self.assertGreater(result["pss_mib"], 0)
        self.assertGreaterEqual(result["rss_mib"], result["pss_mib"])
        self.assertGreaterEqual(result["pss_mib"], result["private_mib"])
        self.assertGreaterEqual(result["cpu_seconds"], 0)

    def test_snapshot_ignores_missing_process(self):
        self.assertEqual(snapshot([999999999])["processes"], 0)

    def test_application_echo_not_terminal_driver_echo(self):
        # Child explicitly reads and renders input. PTY ECHO is disabled.
        code = "import os; os.write(1, b'ready\\r\\n'); data=os.read(0, 100); os.write(1, data); import time; time.sleep(5)"
        terminal = Terminal([sys.executable, "-c", code], os.environ.copy(), Path.cwd())
        try:
            self.assertLess(echo([terminal], terminal, "testprobe", timeout=2), 2000)
            self.assertIn("testprobe", terminal.text())
        finally:
            stop(terminal.proc)
            os.close(terminal.fd)

    def test_unread_input_does_not_count_as_ready(self):
        terminal = Terminal([sys.executable, "-c", "import time; time.sleep(5)"], os.environ.copy(), Path.cwd())
        try:
            with self.assertRaisesRegex(RuntimeError, "Input echo failed"):
                echo([terminal], terminal, "not-echoed", timeout=0.05)
        finally:
            stop(terminal.proc)
            os.close(terminal.fd)

    def test_stop_reaps_owned_process(self):
        with tempfile.TemporaryFile() as log:
            proc = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], stdout=log, stderr=log, start_new_session=True)
            stop(proc)
            self.assertIsNotNone(proc.poll())


if __name__ == "__main__":
    unittest.main()
