"""Contracts for independent CI gates and fallible compiler discovery."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WorkflowGuardTests(unittest.TestCase):
    def test_quality_gates_report_failures_without_weakening_them(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        quality = workflow.split('  quality:\n', 1)[1].split('\n  release-automation:', 1)[0]
        steps = re.split(r'^      - name: ', quality, flags=re.MULTILINE)[1:]
        self.assertGreaterEqual(len(steps), 15)
        for step in steps:
            name = step.splitlines()[0]
            with self.subTest(step=name):
                # The initial module-resolution check runs normally. Every
                # independent check after it must run after earlier failures,
                # stop on cancellation, and retain its failing exit status.
                if name != 'Check module declarations resolve':
                    self.assertIn('if: ${{ !cancelled() }}', step)
                self.assertNotRegex(step, r'\bcontinue-on-error:\s*true\b')
        self.assertNotIn('continue-on-error: true', quality)
        self.assertIn('cargo clippy --all-targets --all-features -- -D warnings', quality)
        for script in ('check_code_size_budget.py', 'check_test_size_budget.py',
                       'check_panic_budget.py', 'check_swallowed_error_budget.py'):
            self.assertIn(f'run: python3 scripts/{script}\n', quality)

    def test_compiler_lookup_failure_is_not_masked_by_export(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        match = re.search(r'^          (RUSTC=.*)\n          (export RUSTC)\n'
                          r'          (CARGO_BIN=.*)\n', workflow, re.MULTILINE)
        self.assertIsNotNone(match, 'compiler discovery must be separate from export')
        fragment = '\n'.join(match.groups()) + '\nprintf should-not-run\n'
        with tempfile.TemporaryDirectory() as temp:
            rustup = Path(temp) / 'rustup'
            rustup.write_text('#!/bin/sh\nexit 17\n')
            rustup.chmod(0o755)
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith('BASH_FUNC_') and key != 'BASH_ENV'}
            env['PATH'] = f'{temp}:{os.environ["PATH"]}'
            result = subprocess.run(['/bin/bash', '-eu', '-c', fragment],
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 17, result.stderr)
            self.assertNotIn('should-not-run', result.stdout)


if __name__ == '__main__':
    unittest.main()
