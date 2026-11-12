#!/usr/bin/env python3
"""Acceptance tests against a built CLI, without completing OAuth.

Run: python3 tests/test_login_qr_cli.py /absolute/path/to/jcode
Temporary homes prevent touching real credentials. No login is completed and
no browser is launched. The tests cover both piped stdin and a terminal.
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from urllib.parse import urlparse

BINARY = str(Path(sys.argv.pop(1)).resolve())
BLOCKS = "█▀▄"


class LoginQrCliTests(unittest.TestCase):
    def run_login(self, flags, *, terminal=False, qr_opt_in=False):
        with tempfile.TemporaryDirectory(
            prefix="jcode-qr-cli-", dir=os.environ.get("JCODE_SCRATCH_DIR")
        ) as home:
            # Deliberately omit browser/display, auth, and QR environment flags.
            env = {
                "PATH": os.environ.get("PATH", ""),
                "HOME": home,
                "JCODE_HOME": str(Path(home) / "jcode"),
                "XDG_CONFIG_HOME": str(Path(home) / "config"),
                "DO_NOT_TRACK": "1",
                "TERM": "xterm-256color",
            }
            if qr_opt_in:
                env["JCODE_SHOW_LOGIN_QR"] = "1"
            command = [BINARY, "login", "--provider", "openai", *flags]
            if terminal:
                import pty

                master, slave = pty.openpty()
                try:
                    result = subprocess.run(
                        command, stdin=slave, capture_output=True, text=True,
                        env=env, timeout=20,
                    )
                finally:
                    os.close(slave)
                    os.close(master)
            else:
                result = subprocess.run(
                    command, input="", capture_output=True, text=True,
                    env=env, timeout=20,
                )
            self.assertEqual(result.returncode, 0, "login prompt failed")
            return result

    def assert_human_qr(self, result):
        self.assertEqual(len(result.stdout.strip().splitlines()), 1)
        self.assertEqual(urlparse(result.stdout.strip()).hostname, "auth.openai.com")
        self.assertIn("Scan this QR", result.stderr)
        self.assertTrue(any(c in result.stderr for c in BLOCKS), "QR glyphs missing")
        self.assertIn("--callback-url", result.stderr)
        self.assertNotIn("Waiting up to", result.stderr)

    def test_browserless_flags_print_qr_without_opt_in(self):
        for flag in ("--no-browser", "--headless"):
            for terminal in (False, True):
                if terminal and os.name != "posix":
                    continue
                with self.subTest(flag=flag, terminal=terminal):
                    self.assert_human_qr(self.run_login([flag], terminal=terminal))

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux display-less detection")
    def test_detected_headless_environment_prints_qr(self):
        self.assert_human_qr(self.run_login([]))
        if os.name == "posix":
            self.assert_human_qr(self.run_login([], terminal=True))

    def test_json_remains_machine_readable_without_qr(self):
        for flags in (
            ["--no-browser", "--json"],
            ["--no-browser", "--print-auth-url", "--json"],
        ):
            with self.subTest(flags=flags):
                result = self.run_login(flags, qr_opt_in=True)
                prompt = json.loads(result.stdout)
                self.assertEqual(prompt["status"], "pending")
                self.assertEqual(prompt["provider"], "openai")
                self.assertEqual(urlparse(prompt["auth_url"]).hostname, "auth.openai.com")
                self.assertFalse(any(c in result.stdout + result.stderr for c in BLOCKS))


if __name__ == "__main__":
    unittest.main(verbosity=2)
