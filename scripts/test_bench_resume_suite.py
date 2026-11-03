"""Tests for resume benchmark output collection, without running Rust builds."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("bench_resume_suite", Path(__file__).with_name("bench_resume_suite.py"))
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


class BenchmarkRowsTests(unittest.TestCase):
    def test_collects_both_schemas_with_test_runner_prefix(self):
        text = '\n'.join([
            'test matrix ... resume_bench {"scenario":"cold","metrics":{}}',
            '{"cache":"warm","resume_bench":1,"scenario":"disk"}',
            'compiler warning: not a measurement',
        ])
        self.assertEqual([r['scenario'] for r in suite.benchmark_rows(text)], ['cold', 'disk'])

    def test_malformed_and_unrelated_output_is_not_a_result(self):
        self.assertEqual(suite.benchmark_rows('resume_bench broken\nresume_bench {"warning":1}\n'), [])

    def test_rejects_lost_hosted_sample_configuration(self):
        with self.assertRaises(RuntimeError):
            suite.validate_sample_counts([{'scenario': 'cold', 'metrics': {'frame': {'samples': 20}}}], 5)
        suite.validate_sample_counts([{'scenario': 'cold', 'metrics': {'frame': {'samples': 5}}}], 5)
        suite.validate_sample_counts([{'scenario': 'disk', 'attempts': 5, 'failures': 5}], 5)

    def test_failed_measurements_keep_null_percentiles(self):
        row = suite.benchmark_rows('resume_bench {"scenario":"failed","p50_ms":null,"failures":2}')[0]
        self.assertIsNone(row['p50_ms'])
        self.assertEqual(row['failures'], 2)


if __name__ == '__main__':
    unittest.main()
