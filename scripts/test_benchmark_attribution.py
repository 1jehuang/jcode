#!/usr/bin/env python3
"""Offline unit tests for scripts/benchmark_attribution.py."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import benchmark_attribution as ba  # noqa: E402

MARKER = "via=jcode-discovery"


def sponsor(**overrides):
    base = {
        "tool": "example",
        "category": "databases",
        "mechanism": "referral-link",
        "marker": MARKER,
    }
    base.update(overrides)
    return base


class ExpectationLoadingTests(unittest.TestCase):
    def test_checked_in_sponsor_file_loads_and_matches_benchmark_cases(self):
        _, sponsors = ba.load_sponsor_expectations(ba.DEFAULT_SPONSORS)
        self.assertTrue(sponsors)
        tools = {s["tool"] for s in sponsors}
        cases = json.loads(
            (ba.REPO_ROOT / "scripts" / "discovery_benchmark_cases.json").read_text()
        )
        case_tools = {
            c["expected_tool"]
            for c in cases["cases"]
            if c.get("expected_tool")
        }
        self.assertEqual(
            case_tools - tools,
            set(),
            "every benchmarked catalog tool needs an attribution expectation",
        )

    def test_rejects_unknown_mechanism(self):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"version": 1, "sponsors": [sponsor(mechanism="vibes")]}, f)
        with self.assertRaises(ba.AttributionError):
            ba.load_sponsor_expectations(Path(f.name))

    def test_rejects_missing_marker(self):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"version": 1, "sponsors": [sponsor(marker="")]}, f)
        with self.assertRaises(ba.AttributionError):
            ba.load_sponsor_expectations(Path(f.name))


class CheckSponsorTests(unittest.TestCase):
    def run_checks(self, entry, spec=None):
        return ba.check_sponsor(spec or sponsor(), entry, live_web=False, timeout=1.0)

    def status(self, report, name):
        return next(c.status for c in report.checks if c.name == name)

    def test_marked_url_and_clean_setup_scores_100(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": f"Sign up at https://example.com/signup?{MARKER} then run `npx -y example-mcp@1.0.0`.",
        }
        report = self.run_checks(entry)
        self.assertEqual(report.score, 100)
        self.assertEqual(self.status(report, "cli_flow_attributable"), "pass")
        self.assertEqual(report.cli_verdict, "attributed")

    def test_missing_entry_fails(self):
        report = self.run_checks(None)
        self.assertEqual(report.score, 0)

    def test_unmarked_listing_url_fails(self):
        report = self.run_checks({"url": "https://example.com/"})
        self.assertEqual(self.status(report, "listing_url_marked"), "fail")
        self.assertLess(report.score, 100)

    def test_setup_with_unmarked_vendor_signup_url_fails(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": "Create an account at https://app.example.com/signup then paste the API key.",
        }
        report = self.run_checks(entry)
        self.assertEqual(self.status(report, "setup_preserves_marker"), "fail")

    def test_cli_only_cookie_attribution_fails(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": "Run `npx -y example-mcp@1.0.0` and paste your API key.",
        }
        report = self.run_checks(entry)
        self.assertEqual(self.status(report, "cli_flow_attributable"), "fail")

    def test_cli_flow_with_non_cookie_mechanism_passes_when_setup_contains_marker(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": "Run `example signup --via jcode-discovery`, then install the CLI.",
        }
        report = self.run_checks(
            entry,
            sponsor(mechanism="cli-flag", marker="--via jcode-discovery", listing_marker=MARKER),
        )
        self.assertEqual(self.status(report, "cli_flow_attributable"), "pass")

    def test_non_cookie_declaration_without_marker_fails(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": "Run `example signup`, then install the CLI.",
        }
        report = self.run_checks(
            entry,
            sponsor(mechanism="cli-flag", marker="--via jcode-discovery", listing_marker=MARKER),
        )
        self.assertEqual(self.status(report, "cli_flow_attributable"), "fail")

    def test_non_cli_setup_skips_cli_check(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": f"Open https://example.com/dashboard?{MARKER} and follow the wizard.",
        }
        report = self.run_checks(entry)
        self.assertEqual(self.status(report, "cli_flow_attributable"), "skip")

    def test_third_party_urls_do_not_trip_setup_check(self):
        entry = {
            "url": f"https://example.com/?{MARKER}",
            "setup": "Docs at https://docs.other-vendor.com/quickstart. Run `pip install example`.",
        }
        report = self.run_checks(entry)
        self.assertEqual(self.status(report, "setup_preserves_marker"), "pass")


class CliAttributionPrimacyTests(unittest.TestCase):
    """CLI-flow attribution is the headline signal and must dominate scoring."""

    def report_for(self, setup, spec=None):
        entry = {"url": f"https://example.com/?{MARKER}", "setup": setup}
        return ba.check_sponsor(spec or sponsor(), entry, live_web=False, timeout=1.0)

    def test_cli_failure_verdict_is_not_attributed(self):
        report = self.report_for("Run `npx -y example-mcp@1.0.0` and paste your API key.")
        self.assertEqual(report.cli_verdict, "NOT-ATTRIBUTED")
        self.assertIn("not be attributed", report.cli_detail)

    def test_cli_skip_is_reported_as_unknown(self):
        report = self.report_for(f"Open https://example.com/dashboard?{MARKER}.")
        self.assertEqual(report.cli_verdict, "unknown")

    def test_missing_cli_check_is_unknown_not_crash(self):
        report = ba.SponsorReport(tool="x", category="y", mechanism="referral-link")
        self.assertEqual(report.cli_verdict, "unknown")

    def test_cli_failure_is_weighted_below_a_single_other_failure(self):
        cli_fail = self.report_for("Run `npx -y example-mcp@1.0.0` and paste your API key.")
        other_fail = ba.check_sponsor(
            sponsor(),
            {
                "url": "https://example.com/",
                "setup": f"Sign up at https://example.com/signup?{MARKER} then run `npx -y x`.",
            },
            live_web=False,
            timeout=1.0,
        )
        self.assertEqual(other_fail.cli_verdict, "attributed")
        self.assertLess(
            cli_fail.score,
            other_fail.score,
            "a CLI-attribution failure must cost more than one ordinary check",
        )


class CatalogLoadingTests(unittest.TestCase):
    def test_loads_nested_and_flat_catalog_shapes(self):
        payload = {
            "categories": {
                "databases": {"tools": [{"name": "Example", "url": "https://example.com"}]},
                "payments": [{"name": "card", "url": "https://card.example"}],
            }
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump(payload, f)
        entries = ba.load_catalog_entries(Path(f.name))
        self.assertIn("example", entries)
        self.assertEqual(entries["card"]["category"], "payments")


if __name__ == "__main__":
    unittest.main()
