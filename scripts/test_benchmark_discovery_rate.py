#!/usr/bin/env python3
"""Offline tests for the Discovery call-rate benchmark.

These cover the parts that decide whether a trial counts and how it is scored,
so the benchmark can be trusted without spending model credits. Run:

    python scripts/test_benchmark_discovery_rate.py
"""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import benchmark_discovery_rate as rate  # noqa: E402


class ExecutableIdentityTests(unittest.TestCase):
    def test_provenance_bearing_reports_use_version_two(self) -> None:
        self.assertEqual(2, rate.REPORT_VERSION)

    def test_records_exact_path_version_commit_and_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            executable = Path(temp_dir) / "fake-jcode"
            content = "#!/bin/sh\nprintf 'jcode v9.9.9 (abcdef123)\\n'\n"
            executable.write_text(content, encoding="utf-8")
            executable.chmod(0o755)

            identity = rate.executable_identity(str(executable))

            self.assertEqual(str(executable.resolve()), identity["path"])
            self.assertEqual("jcode v9.9.9 (abcdef123)", identity["version"])
            self.assertEqual("abcdef123", identity["commit"])
            self.assertEqual(
                hashlib.sha256(content.encode()).hexdigest(), identity["sha256"]
            )
            self.assertEqual(len(content.encode()), identity["size_bytes"])

    def test_extracts_base_commit_from_dirty_build_version(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            executable = Path(temp_dir) / "dirty-jcode"
            executable.write_text(
                "#!/bin/sh\necho 'jcode v0.70.29-dev (bd64f8f82-dirty-d7c46d086c9f)'\n",
                encoding="utf-8",
            )
            executable.chmod(0o755)

            identity = rate.executable_identity(str(executable))

            self.assertEqual("bd64f8f82", identity["commit"])
            self.assertIn("-dirty-d7c46d086c9f", identity["version"])

    def test_rejects_missing_executable(self) -> None:
        with self.assertRaises(rate.BenchmarkError):
            rate.executable_identity("/definitely/missing/jcode")


class DetectBypassTests(unittest.TestCase):
    def test_real_commitments_are_flagged(self) -> None:
        cases = [
            ("bash", '{"command": "npm install @vercel/blob"}', "package-install"),
            ("bash", '{"command": "pip install stripe"}', "package-install"),
            ("bash", '{"command": "cargo add aws-sdk-s3"}', "package-install"),
            ("bash", '{"command": "uv add httpx"}', "package-install"),
            ("bash", '{"command": "npm install @vercel/blob 2>&1"}', "package-install"),
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
            # A redirection is not a package name. This fired as a false
            # positive against a real Claude trial before the pattern required
            # an actual argument.
            ("bash", '{"command": "npm install 2>&1 | tail -3"}'),
            ("bash", '{"command": "npm install --production"}'),
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
        selections = [case for case in cases if case.expect == "select"]
        self.assertGreaterEqual(len(calls), 15)
        self.assertGreaterEqual(len(controls), 8, "controls guard against over-triggering")
        self.assertEqual(
            {(case.expected_tool, case.expected_listed) for case in selections},
            {("context.dev", True), ("firecrawl", False)},
        )
        self.assertTrue(all(case.expected_category == "web-data" for case in selections))
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

    def test_selection_case_requires_complete_expected_receipt(self) -> None:
        base = {
            "id": "x",
            "expect": "select",
            "prompt": "I chose ExampleCo. Set it up.",
            "expected_category": "web-data",
            "expected_tool": "exampleco",
            "expected_listed": False,
        }
        for missing in ("expected_category", "expected_tool", "expected_listed"):
            malformed = dict(base)
            malformed.pop(missing)
            with self.subTest(missing=missing), self.assertRaises(rate.BenchmarkError):
                rate.load_cases(self._write([malformed]), self.categories)

    def test_selection_case_requires_boolean_listed_status(self) -> None:
        path = self._write(
            [
                {
                    "id": "x",
                    "expect": "select",
                    "prompt": "I chose ExampleCo. Set it up.",
                    "expected_category": "web-data",
                    "expected_tool": "exampleco",
                    "expected_listed": "false",
                }
            ]
        )
        with self.assertRaisesRegex(rate.BenchmarkError, "boolean expected_listed"):
            rate.load_cases(path, self.categories)

    def test_call_cases_cannot_declare_selection_fields(self) -> None:
        path = self._write(
            [
                {
                    "id": "x",
                    "expect": "call",
                    "prompt": "Give this application an external capability.",
                    "expected_tool": "exampleco",
                }
            ]
        )
        with self.assertRaisesRegex(rate.BenchmarkError, "only select cases"):
            rate.load_cases(path, self.categories)


class SelectionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.case = rate.RateCase(
            id="select-context",
            expect="select",
            prompt="I chose context.dev. Set it up.",
            expected_category="web-data",
            expected_tool="context.dev",
            expected_listed=True,
        )
        self.call = rate.parse_discovery_output(
            "Selected 'context.dev' from 'web-data' (Jcode tool directory):", 1.0
        )

    def test_tool_input_parser_normalizes_selection(self) -> None:
        self.assertEqual(
            (
                "select",
                "context.dev",
                "The user chose it because it best fits the website enrichment workflow.",
            ),
            rate.parse_tool_input(
                '{"action":"SELECT","tool":"Context.Dev","reason":"The user chose it because it best fits the website enrichment workflow."}'
            ),
        )
        self.assertEqual((None, None, None), rate.parse_tool_input("not json"))
        self.assertEqual((None, None, None), rate.parse_tool_input("[]"))

    def test_exact_input_and_output_match_is_correct(self) -> None:
        self.assertTrue(
            rate.selection_is_correct(
                self.case,
                self.call,
                "select",
                "context.dev",
                "The user explicitly chose context.dev for this website enrichment workflow.",
            )
        )

    def test_selection_requires_action_tool_category_and_listed_status(self) -> None:
        wrong_outputs = [
            rate.parse_discovery_output(
                "Selected 'context.dev' from 'web-search' (Jcode tool directory):", 1.0
            ),
            rate.parse_discovery_output(
                "Selected off-catalog product 'context.dev' for 'web-data'.", 1.0
            ),
            rate.parse_discovery_output(
                "Selected 'another-tool' from 'web-data' (Jcode tool directory):", 1.0
            ),
        ]
        good_reason = (
            "The user explicitly chose context.dev for this website enrichment workflow."
        )
        for action, tool, reason, call in [
            ("search", "context.dev", good_reason, self.call),
            ("select", "firecrawl", good_reason, self.call),
            ("select", "context.dev", None, self.call),
            ("select", "context.dev", "too short", self.call),
            *[("select", "context.dev", good_reason, call) for call in wrong_outputs],
        ]:
            with self.subTest(action=action, tool=tool, reason=reason, output=call.output):
                self.assertFalse(
                    rate.selection_is_correct(self.case, call, action, tool, reason)
                )

    def test_off_catalog_selection_can_match(self) -> None:
        case = rate.RateCase(
            id="select-firecrawl",
            expect="select",
            prompt="I chose Firecrawl. Set it up.",
            expected_category="web-data",
            expected_tool="firecrawl",
            expected_listed=False,
        )
        call = rate.parse_discovery_output(
            "Selected off-catalog product 'Firecrawl' for 'web-data'.", 1.0
        )
        self.assertTrue(
            rate.selection_is_correct(
                case,
                call,
                "select",
                "firecrawl",
                "The user explicitly chose Firecrawl for this website enrichment workflow.",
            )
        )

    def test_any_selection_stops_all_case_kinds_immediately(self) -> None:
        call_case = rate.RateCase("call", "call", "p", "payments")
        control = rate.RateCase("control", "no-call", "p")
        self.assertTrue(rate.discovery_call_stops_trial(self.case, True))
        self.assertTrue(rate.discovery_call_stops_trial(call_case, True))
        self.assertTrue(rate.discovery_call_stops_trial(control, False))
        self.assertFalse(rate.discovery_call_stops_trial(call_case, False))


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

    def test_selection_accuracy_counts_missing_and_incorrect_selections(self) -> None:
        case = rate.RateCase(
            "selection", "select", "p", "web-data", "context.dev", True
        )
        trials = [
            self._trial(outcome="selected", selection_correct=True),
            self._trial(trial=2, outcome="incorrect-selection", selection_correct=False),
            self._trial(trial=3, outcome="no-selection", browsed=False),
        ]
        summary = rate.summarize_case(case, trials)
        self.assertAlmostEqual(1 / 3, summary["selection_accuracy"])
        self.assertFalse(summary["passed"])

    def test_selection_only_aggregate_passes_and_fails_its_own_gate(self) -> None:
        case = rate.RateCase(
            "selection", "select", "p", "web-data", "context.dev", True
        )
        result = rate.summarize_case(
            case,
            [
                self._trial(
                    outcome="selected",
                    selection_correct=True,
                    selected_via_discovery=["context.dev"],
                )
            ],
        )
        aggregate = rate.aggregate([result])
        self.assertIsNone(aggregate["recall_browse_rate"])
        self.assertIsNone(aggregate["control_clean_rate"])
        self.assertEqual(1.0, aggregate["select_rate"])
        self.assertEqual(1.0, aggregate["selection_accuracy"])
        self.assertTrue(rate.passes_gates(aggregate, 0.99, 0.99, 1.0))

        aggregate["selection_accuracy"] = 0.5
        self.assertFalse(rate.passes_gates(aggregate, 0.0, 0.0, 0.75))

    def test_no_scored_trials_never_pass_any_filtered_gate(self) -> None:
        summary = rate.aggregate(
            [
                rate.summarize_case(
                    self._case(),
                    [self._trial(outcome="invalid", browsed=False, invalid_reason="quota")],
                )
            ]
        )
        self.assertFalse(rate.passes_gates(summary, 0.0, 0.0, 0.0))


if __name__ == "__main__":
    unittest.main(verbosity=2)
