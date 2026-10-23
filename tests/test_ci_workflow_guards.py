"""Contracts for independent CI gates and fallible compiler discovery."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WorkflowGuardTests(unittest.TestCase):
    def test_ci_builds_and_tests_cannot_silently_refresh_the_lockfile(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        command = re.compile(r'(?:cargo|"\$CARGO_BIN"|"\$\(rustup which cargo\)") '
                             r'(?:build|check|test|clippy)\b')
        checked = 0
        for line in workflow.splitlines():
            if line.lstrip().startswith('#') or not command.search(line):
                continue
            with self.subTest(command=line.strip()):
                self.assertIn('--locked', line)
                checked += 1
        self.assertGreaterEqual(checked, 15)
        self.assertIn('git diff --exit-code -- Cargo.lock', workflow)

    def test_arm64_release_graph_is_a_mandatory_native_windows_check(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        arm64 = workflow.split('  windows-arm64-check:\n', 1)[1].split(
            '\n  windows-cross-check:', 1)[0]
        self.assertIn('runs-on: windows-latest', arm64)
        self.assertIn('arch: amd64_arm64', arm64)
        self.assertIn('targets: aarch64-pc-windows-msvc', arm64)
        self.assertIn('JUSTRUST_TOOLCHAIN: system', arm64)
        self.assertIn('JUSTRUST_DISABLE: "1"', arm64)
        self.assertIn('& justrust check --local --locked -p jcode --bins '
                      '--target aarch64-pc-windows-msvc '
                      '--no-default-features --features pdf', arm64)
        self.assertIn('if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }', arm64)
        self.assertNotIn('continue-on-error:', arm64)
        self.assertNotIn('if:', arm64)

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
        self.assertIn('cargo clippy --locked --all-targets --all-features -- -D warnings', quality)
        for script in ('check_code_size_budget.py', 'check_test_size_budget.py',
                       'check_panic_budget.py', 'check_swallowed_error_budget.py'):
            self.assertIn(f'run: python3 scripts/{script}\n', quality)

    def test_sidepanel_regressions_execute_on_native_platform_jobs(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        native = workflow.split('      - name: Run sidepanel retention and history regressions\n', 1)[1].split('      - name:', 1)[0]
        windows = workflow.split('      - name: Run Windows sidepanel retention and history regressions\n', 1)[1].split('      - name:', 1)[0]
        for package in ('jcode-base', 'jcode-app-core', 'jcode-tui', 'jcode-protocol'):
            self.assertIn(package, native)
            self.assertIn(package, windows)
        self.assertIn('--lib panel -- --test-threads=1', native)
        self.assertIn("'--lib', 'panel', '--', '--test-threads=1", windows)
        self.assertIn('--lib model_catalog_', native)
        self.assertIn('run_with_timeout.py', native)
        self.assertIn('invoke_cargo_with_timeout.ps1', windows)
        for step in (native, windows):
            self.assertNotIn('continue-on-error:', step)
            self.assertNotIn('if:', step)

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
