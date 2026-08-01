#!/usr/bin/env python3
"""Offline tests for the Discovery call-rate benchmark.

These cover the parts that decide whether a trial counts and how it is scored,
so the benchmark can be trusted without spending model credits. Run:

    python scripts/test_benchmark_discovery_rate.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import benchmark_discovery_rate as rate  # noqa: E402


class DetectBypassTests(unittest.TestCase):
    def test_real_commitments_are_flagged(self) -> None:
        cases = [
            ("bash", '{"command": "npm install @vercel/blob"}', "package-install"),
            ("bash", '{"command": "pip install stripe"}', "package-install"),
            ("bash", '{"command": "cargo add aws-sdk-s3"}', "package-install"),
            ("bash", '{"command": "cd app && vercel deploy --prod"}', "vendor-cli"),
            ("bash", '{"command": "npx wrangler r2 bucket create uploads"}', "vendor-cli"),
            ("webfetch", '{"url": "https://api.stripe.com/v1/charges"}', "vendor-endpoint"),
            ("webfetch", '{"url": "https://neon.tech/pricing"}', "signup-page"),
        ]
        for tool, payload, expected in cases:
            with self.subTest(payload=payload):
                kinds = {bypass.kind for bypass in rate.detect_bypasses(tool, payload, 1.0)}
                self.assertIn(expected, kinds)

    def test_local_work_is_not_flagged(self) -> None:
        benign = [
            ("bash", '{"command": "ls -la"}'),
            ("bash", '{"command": "python -m pytest -q"}'),
            ("bash", '{"command": "npm install"}'),  # restore existing lockfile, no vendor chosen
            ("bash", '{"command": "pip install -r requirements.txt"}'),
            ("bash", '{"command": "command -v vercel neonctl psql node"}'),
            ("bash", '{"command": "git log --oneline -5"}'),
            ("bash", '{"command": "cat .env.example"}'),
            # A vendor name inside written file content is not a command.
            ("bash", '{"command": "cat > notes.md <<EOF\\nNeon Postgres is an option\\nEOF"}'),
            ("read", '{"file_path": "src/vercel.ts"}'),
            ("write", '{"content": "import Stripe from stripe"}'),
        ]
        for tool, payload in benign:
            with self.subTest(payload=payload):
                found = rate.detect_bypasses(tool, payload, 1.0)
                self.assertEqual([], found, f"unexpected bypass: {found}")

    def test_only_scanned_tools_participate(self) -> None:
        self.assertEqual([], rate.detect_bypasses("agentgrep", "npm install stripe", 1.0))

    def test_mcp_connect_counts_as_bypass(self) -> None:
        kinds = {
            bypass.kind
            for bypass in rate.detect_bypasses("mcp", '{"action": "connect", "server": "x"}', 1.0)
        }
        self.assertEqual({"mcp-connect"}, kinds)


class InvalidTrialTests(unittest.TestCase):
    def test_provider_failures_are_recognized(self) -> None:
        failures = [
            "Error: OpenAI token refresh failed; run /login to re-authenticate",
            'status: 402 Payment Required {"error":"insufficient credits"}',
            'status: 400 Bad Request {"code":"request_too_expensive"}',
            "status: 429 rate limit exceeded",
            "Error: unknown model gpt-nope",
            "error sending request: failed to connect",
        ]
        for text in failures:
            with self.subTest(text=text):
                self.assertIsNotNone(rate.INVALID_STDERR_RE.search(text))

    def test_ordinary_agent_noise_is_not_invalid(self) -> None:
        for text in ["tool bash exited with status 1", "warning: unused variable", ""]:
            with self.subTest(text=text):
                self.assertIsNone(rate.INVALID_STDERR_RE.search(text))


class CaseFileTests(unittest.TestCase):
    def setUp(self) -> None:
        self.categories = rate.load_categories()

    def test_shipped_suite_is_valid_and_balanced(self) -> None:
        cases = rate.load_cases(rate.DEFAULT_CASES, self.categories)
        calls = [case for case in cases if case.expect == "call"]
        controls = [case for case in cases if case.expect == "no-call"]
        self.assertGreaterEqual(len(calls), 15)
        self.assertGreaterEqual(len(controls), 8, "controls guard against over-triggering")
        # Every category with a positive case should be represented at most once
        # per distinct scenario, and all declared categories must be real.
        for case in calls:
            if case.expected_category:
                self.assertIn(case.expected_category, self.categories)

    def test_suite_covers_most_categories(self) -> None:
        cases = rate.load_cases(rate.DEFAULT_CASES, self.categories)
        covered = {case.expected_category for case in cases if case.expected_category}
        missing = set(self.categories) - covered - {"other"}
        self.assertEqual(set(), missing, f"categories with no call case: {sorted(missing)}")

    def _write(self, cases: list[dict]) -> Path:
        path = Path(tempfile.mkdtemp()) / "cases.json"
        path.write_text(json.dumps({"version": 1, "cases": cases}), encoding="utf-8")
        return path

    def test_prompt_leaking_discovery_is_rejected(self) -> None:
        path = self._write(
            [{"id": "x", "expect": "call", "prompt": "please use discover_tools for payments"}]
        )
        with self.assertRaises(rate.BenchmarkError):
            rate.load_cases(path, self.categories)

    def test_prompt_leaking_category_slug_is_rejected(self) -> None:
        path = self._write(
            [
                {
                    "id": "x",
                    "expect": "call",
                    "expected_category": "code-review",
                    "prompt": "set up code review for my repository please now",
                }
            ]
        )
        with self.assertRaises(rate.BenchmarkError):
            rate.load_cases(path, self.categories)

    def test_control_may_not_declare_a_category(self) -> None:
        path = self._write(
            [{"id": "x", "expect": "no-call", "expected_category": "payments", "prompt": "hi there"}]
        )
        with self.assertRaises(rate.BenchmarkError):
            rate.load_cases(path, self.categories)


class ScoringTests(unittest.TestCase):
    def _case(self, expect: str = "call", category: str | None = "payments") -> rate.RateCase:
        return rate.RateCase(id="c", expect=expect, prompt="p", expected_category=category)

    def _trial(self, **kwargs) -> rate.TrialResult:
        base = {"trial": 1, "outcome": "browsed", "browsed": True}
        base.update(kwargs)
        return rate.TrialResult(**base)

    def test_invalid_trials_are_excluded_from_rates(self) -> None:
        case = self._case()
        trials = [
            self._trial(),
            self._trial(trial=2, outcome="invalid", browsed=False, invalid_reason="insufficient"),
        ]
        summary = rate.summarize_case(case, trials)
        self.assertEqual(1, summary["scored_trial_count"])
        self.assertEqual(1, summary["invalid_trial_count"])
        self.assertEqual(1.0, summary["browse_rate"])
        self.assertTrue(summary["passed"])

    def test_all_invalid_case_cannot_pass(self) -> None:
        summary = rate.summarize_case(
            self._case(),
            [self._trial(outcome="invalid", browsed=False, invalid_reason="quota")],
        )
        self.assertFalse(summary["passed"])
        self.assertIsNone(summary["browse_rate"])

    def test_control_false_positive_fails(self) -> None:
        summary = rate.summarize_case(
            self._case(expect="no-call", category=None),
            [self._trial(outcome="false-positive", browsed=True, discovery_calls=[{"outcome": "listing"}])],
        )
        self.assertFalse(summary["passed"])
        self.assertEqual(1.0, summary["call_rate"])

    def test_aggregate_ignores_unscored_cases(self) -> None:
        scored = rate.summarize_case(self._case(), [self._trial()])
        unscored = rate.summarize_case(
            rate.RateCase(id="d", expect="call", prompt="p", expected_category="databases"),
            [self._trial(outcome="invalid", browsed=False, invalid_reason="quota")],
        )
        summary = rate.aggregate([scored, unscored])
        self.assertEqual(1.0, summary["recall_browse_rate"])
        self.assertEqual(1, summary["invalid_trial_count"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
