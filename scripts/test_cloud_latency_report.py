import contextlib
import importlib.util
import io
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("cloud_latency_report", Path(__file__).with_name("cloud_latency_report.py"))
reporter = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reporter)


class CloudLatencyReportTests(unittest.TestCase):
    def test_window_is_inclusive_exclusive_and_handles_mixed_logs(self):
        lines = [
            "[2026-11-07 23:59:59.999] [CLOUD_TIMING] stage=old duration_ms=9000",
            "[2026-11-08 00:00:00.000] [CLOUD_TIMING] stage=new duration_ms=20",
            "[2026-11-08 00:01:00.000] [CLOUD_TIMING] stage=later duration_ms=30",
            "[CLOUD_TIMING] stage=undated duration_ms=9999",
            "[2026-13-08 00:00:00] [CLOUD_TIMING] stage=invalid duration_ms=9999",
        ]
        since = reporter.local_timestamp("2026-11-08T00:00:00")
        until = reporter.local_timestamp("2026-11-08T00:01:00")
        self.assertEqual(list(reporter.in_window(lines, since, until)), [lines[1]])
        self.assertEqual(list(reporter.in_window(lines)), lines)
        self.assertEqual(list(reporter.in_window(lines[:3], since=since)), lines[1:3])
        self.assertEqual(list(reporter.in_window(lines[:3], until=until)), lines[:2])

    def test_window_rejects_timezone_ambiguity_and_invalid_dates(self):
        import argparse
        for value in ["invalid", "2026-11-08T00:00:00Z", "2026-11-08T00:00:00-08:00"]:
            with self.assertRaises(argparse.ArgumentTypeError):
                reporter.local_timestamp(value)

    def test_collects_failure_and_secondary_wait_metrics(self):
        groups, failures = reporter.collect([
            "[INFO] [CLOUD_TIMING] stage=event_parse operation=history outcome=ok duration_ms=12.5 request_wait_ms=1400 bytes=261000\n",
            "[INFO] [CLOUD_TIMING] stage=event_parse operation=history outcome=error duration_ms=2\n",
            "[INFO] mobile_sync stage=http_upload outcome=timeout duration_ms=6000\n",
        ])
        self.assertEqual(groups[("event_parse:history", "duration_ms")], [12.5, 2])
        self.assertEqual(groups[("event_parse:history", "request_wait_ms")], [1400])
        self.assertEqual(failures[("event_parse:history", "duration_ms")], 1)
        self.assertEqual(failures[("http_upload", "duration_ms")], 1)

    def test_ignores_unrelated_invalid_and_nonfinite_values(self):
        groups, _ = reporter.collect([
            "private stage=anything outcome=ok duration_ms=9000",
            "[CLOUD_TIMING] stage=https://secret outcome=ok duration_ms=9",
            "[CLOUD_TIMING] stage=parse outcome=ok duration_ms=nan",
            "[CLOUD_TIMING] stage=parse outcome=ok duration_ms=inf",
            "[CLOUD_TIMING] stage=parse outcome=ok request_wait_ms=none",
            "[CLOUD_TIMING] stage=parse outcome=ok duration_ms=" + "9" * 400,
        ])
        self.assertFalse(groups)

    def test_counts_http_body_and_projection_failures_not_normal_skips(self):
        bad = ["http_error", "load_error", "read_error", "size_limit", "oversized", "identity_mismatch", "exit_failure", "rejected", "undelivered"]
        normal = ["success", "miss", "skipped_absent", "skipped_disabled", "success_cache_unavailable", "no_devices"]
        groups, failures = reporter.collect([
            f"[CLOUD_TIMING] stage=probe outcome={outcome} duration_ms=10"
            for outcome in bad + normal
        ])
        self.assertEqual(len(groups[("probe", "duration_ms")]), len(bad + normal))
        self.assertEqual(failures[("probe", "duration_ms")], len(bad))

    def test_report_is_ranked_filtered_and_content_free(self):
        groups, failures = reporter.collect([
            "[CLOUD_TIMING] stage=fast outcome=ok duration_ms=20 payload=PRIVATE",
            "[CLOUD_TIMING] stage=slow outcome=error duration_ms=1000 payload=PRIVATE",
            "[CLOUD_TIMING] stage=slower outcome=ok duration_ms=3000 token=SECRET",
        ])
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            reporter.report(groups, failures, threshold_ms=500)
        text = output.getvalue()
        self.assertLess(text.index("slower"), text.index("slow "))
        self.assertNotIn("fast", text)
        self.assertNotIn("PRIVATE", text)
        self.assertNotIn("SECRET", text)
        self.assertEqual(reporter.percentile([1, 2, 3, 4], .95), 4)


if __name__ == "__main__":
    unittest.main()
