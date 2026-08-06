#!/usr/bin/env python3
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("benchmark_discovery.py")
SPEC = importlib.util.spec_from_file_location("benchmark_discovery", SCRIPT)
assert SPEC and SPEC.loader
benchmark = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = benchmark
SPEC.loader.exec_module(benchmark)


class DiscoveryBenchmarkTests(unittest.TestCase):
    def test_categories_are_loaded_from_rust_source(self):
        categories = benchmark.load_categories()
        self.assertIn("payments", categories)
        self.assertIn("web-data", categories)
        self.assertEqual(len(categories), len(set(categories)))

    def test_checked_in_cases_are_natural_and_unique(self):
        cases = benchmark.load_cases(benchmark.DEFAULT_CASES)
        self.assertEqual(
            {
                (case.expected_category, case.expected_tool)
                for case in cases
                if case.expectation == "listing"
            },
            {
                ("payments", "agentcard"),
                ("code-review", "greptile"),
                ("web-data", "context.dev"),
                ("email-messaging", "agentmail"),
            },
        )
        self.assertEqual(
            sum(case.expected_tool == "agentmail" for case in cases),
            3,
        )
        self.assertEqual(
            sum(case.expectation == "no-discovery" for case in cases),
            2,
        )

    def test_case_loader_rejects_expected_tool_leakage(self):
        payload = {
            "version": 1,
            "cases": [
                {
                    "id": "bad",
                    "expected_category": "payments",
                    "expected_tool": "agentcard",
                    "prompt": "Please use Agentcard for this purchase.",
                }
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cases.json"
            path.write_text(json.dumps(payload), encoding="utf-8")
            with self.assertRaisesRegex(benchmark.BenchmarkError, "leaks"):
                benchmark.load_cases(path)

    def test_case_loader_accepts_no_discovery_controls(self):
        payload = {
            "version": 2,
            "cases": [
                {
                    "id": "draft-only",
                    "expectation": "no-discovery",
                    "prompt": "Draft a short message but do not send it.",
                }
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cases.json"
            path.write_text(json.dumps(payload), encoding="utf-8")
            cases = benchmark.load_cases(path)
        self.assertEqual(cases[0].expectation, "no-discovery")
        self.assertIsNone(cases[0].expected_category)
        self.assertIsNone(cases[0].expected_tool)

    def test_case_loader_rejects_target_on_no_discovery_control(self):
        payload = {
            "version": 2,
            "cases": [
                {
                    "id": "bad-control",
                    "expectation": "no-discovery",
                    "expected_category": "email-messaging",
                    "expected_tool": "agentmail",
                    "prompt": "Draft a short message but do not send it.",
                }
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cases.json"
            path.write_text(json.dumps(payload), encoding="utf-8")
            with self.assertRaisesRegex(benchmark.BenchmarkError, "must not declare"):
                benchmark.load_cases(path)

    def test_catalog_coverage_reports_missing_and_stale_cases(self):
        cases = [
            benchmark.BenchmarkCase("agent", "payments", "agentcard", "Buy an item."),
            benchmark.BenchmarkCase("stale", "other", "old-tool", "Complete a task."),
        ]
        catalog = {
            "payments": [{"name": "agentcard"}],
            "web-data": [{"name": "context.dev"}],
        }
        coverage = benchmark.validate_catalog_coverage(cases, catalog)
        self.assertEqual(coverage["missing_cases"], ["web-data/context.dev"])
        self.assertEqual(coverage["stale_cases"], ["other/old-tool"])

    def test_parse_listing_extracts_category_and_tools(self):
        output = """Discoverable tools in 'payments' (sponsored discovery):

- agentcard: prepaid virtual Visa cards
- second-tool: another option
"""
        call = benchmark.parse_discovery_output(output, 1.25)
        self.assertEqual(call.category, "payments")
        self.assertEqual(call.tools, ["agentcard", "second-tool"])
        self.assertEqual(call.outcome, "listing")

    def test_parse_empty_category(self):
        call = benchmark.parse_discovery_output(
            "No discoverable tools in category 'browser-automation' right now.", 2.0
        )
        self.assertEqual(call.category, "browser-automation")
        self.assertEqual(call.tools, [])
        self.assertEqual(call.outcome, "empty")

    def test_parse_accepts_current_integration_vocabulary(self):
        """The renderers were renamed from discovery to integration wording.
        Both vocabularies must parse so pre-rename baselines stay comparable
        with post-rename runs."""
        listing = benchmark.parse_discovery_output(
            "Available integrations in 'payments' (Jcode tool directory):\n\n- agentcard: cards\n",
            1.0,
        )
        self.assertEqual(listing.category, "payments")
        self.assertEqual(listing.tools, ["agentcard"])
        self.assertEqual(listing.outcome, "listing")

        empty = benchmark.parse_discovery_output(
            "No integrations in category 'browser-automation' right now.", 1.0
        )
        self.assertEqual(empty.category, "browser-automation")
        self.assertEqual(empty.outcome, "empty")

        selection = benchmark.parse_discovery_output(
            "Set up 'agentmail' from 'email-messaging' (Jcode tool directory):", 1.0
        )
        self.assertEqual(selection.category, "email-messaging")
        self.assertEqual(selection.tools, ["agentmail"])
        self.assertEqual(selection.outcome, "selection")
        self.assertIs(selection.listed, True)

    def test_parse_current_catalog_selection_receipt(self):
        call = benchmark.parse_discovery_output(
            "Selected 'context.dev' from 'web-data' (Jcode tool directory; the choice "
            "must be based only on fit; details: https://jcode.sh/discovery-tools):\n\n"
            "context.dev: structured extraction",
            1.25,
        )
        self.assertEqual(call.category, "web-data")
        self.assertEqual(call.tools, ["context.dev"])
        self.assertEqual(call.outcome, "selection")
        self.assertIs(call.listed, True)

    def test_parse_off_catalog_selection_receipt(self):
        call = benchmark.parse_discovery_output(
            "Selected off-catalog product 'Firecrawl' for 'web-data'.\n\n"
            "Selection recorded as demand data. Jcode does not list or partner with this product.",
            2.5,
        )
        self.assertEqual(call.category, "web-data")
        self.assertEqual(call.tools, ["firecrawl"])
        self.assertEqual(call.outcome, "selection")
        self.assertIs(call.listed, False)

    def test_parse_unmatched_and_error_outputs_without_crashing(self):
        other = benchmark.parse_discovery_output("unexpected renderer output", 1.0)
        self.assertEqual(other.outcome, "other")
        self.assertIsNone(other.category)
        self.assertEqual(other.tools, [])

        error = benchmark.parse_discovery_output("Error: unavailable", 1.0)
        self.assertEqual(error.outcome, "error")

    def test_parse_selection_tracks_but_does_not_count_direct_selection(self):
        call = benchmark.parse_discovery_output(
            "Selected 'agentmail' from 'email-messaging' (Jcode tool directory):", 1.5
        )
        self.assertEqual(call.category, "email-messaging")
        self.assertEqual(call.tools, ["agentmail"])
        self.assertEqual(call.outcome, "selection")
        self.assertIs(call.listed, True)
        case = benchmark.BenchmarkCase(
            "agentmail", "email-messaging", "agentmail", "Set up an inbox."
        )
        self.assertEqual(benchmark.discovery_call_decision(case, call), "failure")

    def test_no_discovery_control_fails_on_any_discovery_call(self):
        case = benchmark.BenchmarkCase(
            "draft-only", None, None, "Draft an email.", "no-discovery"
        )
        call = benchmark.DiscoveryCall(
            elapsed_seconds=1.0,
            category="email-messaging",
            tools=["agentmail"],
            outcome="listing",
            output="",
        )
        self.assertEqual(benchmark.discovery_call_decision(case, call), "failure")

    def test_no_discovery_control_never_retries_after_false_positive(self):
        case = benchmark.BenchmarkCase(
            "draft-only", None, None, "Draft an email.", "no-discovery"
        )
        attempt = benchmark.AttemptResult(
            attempt=1,
            success=False,
            elapsed_seconds=1.0,
            hit_seconds=None,
            exit_code=-15,
            timed_out=False,
            discovery_calls=[],
            runtime_error_count=0,
            stderr_tail="",
        )
        self.assertFalse(benchmark.should_retry(case, attempt))

    def test_case_summary_counts_wrong_categories(self):
        case = benchmark.BenchmarkCase("agent", "payments", "agentcard", "Buy an item.")
        trials = [
            {
                "success": True,
                "attempts_to_hit": 2,
                "hit_seconds": 3.5,
                "attempts": [
                    {
                        "discovery_calls": [
                            {"category": "web-search", "tools": []},
                            {"category": "payments", "tools": ["agentcard"]},
                        ]
                    }
                ],
            }
        ]
        summary = benchmark.summarize_case(case, trials)
        self.assertEqual(summary["success_rate"], 1.0)
        self.assertEqual(summary["first_attempt_success_rate"], 0.0)
        self.assertEqual(summary["first_attempt_target_reach_rate"], 1.0)
        self.assertEqual(summary["mean_attempts_to_hit"], 2)
        self.assertEqual(summary["wrong_category_calls"], {"web-search": 1})
        self.assertEqual(summary["direct_selection_calls"], 0)

    def test_case_summary_marks_runtime_confounded_misses(self):
        case = benchmark.BenchmarkCase("agent", "payments", "agentcard", "Buy an item.")
        trials = [
            {
                "success": False,
                "attempts_to_hit": None,
                "hit_seconds": None,
                "attempts": [
                    {
                        "runtime_error_count": 2,
                        "discovery_calls": [],
                    }
                ],
            }
        ]
        summary = benchmark.summarize_case(case, trials)
        self.assertEqual(summary["runtime_confounded_trials"], 1)
        self.assertEqual(summary["success_rate"], 0.0)


if __name__ == "__main__":
    unittest.main()
