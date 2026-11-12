#!/usr/bin/env python3
"""End-to-end tests for the mock sponsor's public HTTP and CLI interfaces."""

from __future__ import annotations

import json
import re
import shlex
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mock_sponsor_service as mock  # noqa: E402

SCRIPT = Path(__file__).with_name("mock_sponsor_service.py")
BENCHMARK = Path(__file__).with_name("benchmark_attribution.py")
SECRET = b"test-secret"


class MockSponsorEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.db = Path(cls.temp.name) / "sponsor.sqlite"
        cls.server = mock.ThreadingHTTPServer(("127.0.0.1", 0), mock.make_handler(cls.db, SECRET))
        cls.url = mock.server_url(cls.server)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()
        cls.temp.cleanup()

    def cli(self, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            check=check,
            text=True,
            capture_output=True,
        )

    def signup_and_confirm(self, email: str, via: str | None) -> str:
        args = ["signup", "--service", self.url, "--email", email]
        if via is not None:
            args.extend(["--via", via])
        link = self.cli(*args).stdout.strip()
        result = json.loads(self.cli("confirm", link).stdout)
        self.assertTrue(result["confirmed"])
        return link

    def account(self, email: str) -> dict:
        result = self.cli("account", "--service", self.url, "--email", email)
        return json.loads(result.stdout)

    def test_cli_flag_survives_magic_link_and_is_persisted(self):
        email = "attributed@example.test"
        link = self.signup_and_confirm(email, "jcode-discovery")
        self.assertNotIn("jcode-discovery", link, "source should be inside signed state, not a mutable query field")
        self.assertEqual(self.account(email)["acquisition_source"], "jcode-discovery")

    def test_first_acquisition_source_is_immutable(self):
        email = "immutable@example.test"
        self.signup_and_confirm(email, "jcode-discovery")
        self.signup_and_confirm(email, "other-partner")
        self.assertEqual(self.account(email)["acquisition_source"], "jcode-discovery")

    def test_omitted_flag_creates_unattributed_account(self):
        email = "organic@example.test"
        self.signup_and_confirm(email, None)
        self.assertIsNone(self.account(email)["acquisition_source"])

    def test_tampered_magic_link_is_rejected(self):
        result = self.cli(
            "signup", "--service", self.url, "--email", "tamper@example.test", "--via", "jcode-discovery"
        )
        link = result.stdout.strip()
        parsed = urllib.parse.urlsplit(link)
        token = urllib.parse.parse_qs(parsed.query)["token"][0]
        body, signature = token.split(".", 1)
        tampered_body = ("A" if body[0] != "A" else "B") + body[1:]
        tampered_query = urllib.parse.urlencode({"token": f"{tampered_body}.{signature}"})
        tampered = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, parsed.path, tampered_query, ""))
        failed = self.cli("confirm", tampered, check=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("invalid signup token", failed.stderr)

    def test_magic_link_is_one_time_use(self):
        link = self.signup_and_confirm("once@example.test", "jcode-discovery")
        failed = self.cli("confirm", link, check=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("already consumed", failed.stderr)

    def test_expired_token_is_rejected(self):
        token = mock.encode_token({"email": "old@example.test", "via": "jcode-discovery", "exp": 1}, SECRET)
        with self.assertRaisesRegex(ValueError, "expired"):
            mock.decode_token(token, SECRET, now=2)

    def test_real_attribution_benchmark_accepts_discovery_to_cli_contract(self):
        sponsor_file = Path(self.temp.name) / "mock-sponsors.json"
        sponsor_file.write_text(json.dumps({
            "version": 1,
            "default_marker": "via=jcode-discovery",
            "sponsors": [{
                "tool": "mock-sponsor",
                "category": "payments",
                "mechanism": "cli-flag",
                "marker": "--via jcode-discovery",
                "listing_marker": "via=jcode-discovery",
            }],
        }))
        report_file = Path(self.temp.name) / "benchmark-report.json"
        result = subprocess.run(
            [
                sys.executable,
                str(BENCHMARK),
                "--live",
                "--live-web",
                "--endpoint",
                f"{self.url}/v1/discovery",
                "--sponsors",
                str(sponsor_file),
                "--output",
                str(report_file),
            ],
            check=True,
            text=True,
            capture_output=True,
        )
        report = json.loads(report_file.read_text())
        self.assertIn("1/1 sponsors credit agent-driven CLI signups to jcode", result.stdout)
        self.assertEqual(report["sponsors"][0]["score"], 100)
        self.assertEqual(report["sponsors"][0]["cli_attribution"], "attributed")

    def test_exact_discovery_served_command_creates_attributed_account(self):
        query = urllib.parse.urlencode({
            "category": "payments",
            "q": "virtual payment capability for an automated test account",
            "reason": "Acceptance test selects the reference sponsor to validate its public setup contract.",
            "tool": "mock-sponsor",
        })
        with urllib.request.urlopen(f"{self.url}/v1/discovery?{query}") as response:
            selected = json.loads(response.read())["tool"]
        match = re.search(r"`([^`]+)`", selected["setup"])
        self.assertIsNotNone(match, "select setup must contain one executable command")
        email = "served-command@example.test"
        command = shlex.split(match.group(1).replace("<email>", email))
        signup = subprocess.run(command, check=True, text=True, capture_output=True)
        magic_link = signup.stdout.strip()
        confirmed = json.loads(self.cli("confirm", magic_link).stdout)
        self.assertTrue(confirmed["confirmed"])
        self.assertEqual(self.account(email)["acquisition_source"], "jcode-discovery")


if __name__ == "__main__":
    unittest.main(verbosity=2)
