"""Contracts for independent CI gates and fallible compiler discovery."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WorkflowGuardTests(unittest.TestCase):
    def test_npm_duplicate_upload_defers_only_exact_conflict_to_visibility_gate(self):
        workflow = (ROOT / '.github/workflows/publish-typescript-sdk.yml').read_text()
        block = workflow.split('      - name: Publish platform runtime packages\n', 1)[1]
        block = block.split('      - name: Wait for platform runtime registry visibility\n', 1)[0]
        script = textwrap.dedent(block.split('        run: |\n', 1)[1])
        script = script.replace('${{ steps.channel.outputs.tag }}', 'preview')
        for code, message, expected in (
                ('E409', 'Cannot publish over previously staged version "1.2.0-preview.49".', 0),
                ('E409', 'Unrelated conflict', 1),
                ('E404', 'Not authorized', 1)):
            with self.subTest(code=code, message=message), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                package = root / 'sdk/npm/runtime'
                package.mkdir(parents=True)
                (package / 'package.json').write_text(
                    '{"name":"@test/runtime","version":"1.2.0-preview.49"}')
                npm = root / 'npm'
                npm.write_text('#!/bin/sh\nif [ "$1" = view ]; then exit 1; fi\n'
                               'echo "npm error code $ERROR_CODE" >&2\n'
                               'echo "$ERROR_MESSAGE" >&2\nexit 1\n')
                npm.chmod(0o755)
                env = {**os.environ, 'PATH': f'{root}:{os.environ["PATH"]}',
                       'ERROR_CODE': code, 'ERROR_MESSAGE': message}
                result = subprocess.run(['/bin/bash', '-eu', '-c', script],
                                        cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)

    def test_sdk_waits_for_runtime_visibility_and_fails_closed(self):
        workflow = (ROOT / '.github/workflows/publish-typescript-sdk.yml').read_text()
        guard = workflow.split('      - name: Wait for platform runtime registry visibility\n', 1)[1]
        guard, publish = guard.split('      - name: Publish with npm provenance\n', 1)
        script = textwrap.dedent(guard.split('        run: |\n', 1)[1])
        self.assertIn('--prefer-online', script)
        for mode in ('retry', 'missing'):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                package = root / 'sdk/npm/runtime'
                package.mkdir(parents=True)
                (package / 'package.json').write_text(
                    '{"name":"@test/runtime","version":"1.2.0-preview.49"}')
                binaries = root / 'bin'
                binaries.mkdir()
                npm = binaries / 'npm'
                npm.write_text('#!/bin/sh\n'
                               'if [ "$VISIBILITY_MODE" = retry ] && [ -f "$MARKER" ]; then\n'
                               '  echo 1.2.0-preview.49\n  exit 0\nfi\n'
                               'touch "$MARKER"\nexit 1\n')
                npm.chmod(0o755)
                sleep = binaries / 'sleep'
                sleep.write_text('#!/bin/sh\nexit 0\n')
                sleep.chmod(0o755)
                env = {**os.environ, 'PATH': f'{binaries}:{os.environ["PATH"]}',
                       'VISIBILITY_MODE': mode, 'MARKER': str(root / 'attempted')}
                result = subprocess.run(['/bin/bash', '-eu', '-c', script],
                                        cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0 if mode == 'retry' else 1, result.stderr)
                self.assertIn('Waiting for registry visibility', result.stdout)
                if mode == 'missing':
                    self.assertIn('refusing to publish SDK', result.stderr)

    def test_preview_dispatches_standalone_trusted_npm_publisher(self):
        workflow = (ROOT / '.github/workflows/preview.yml').read_text()
        publish = workflow.split('\n  publish-sdk:\n', 1)[1].split('\n  announce:', 1)[0]
        self.assertIn('needs: publish', publish)
        self.assertIn('actions: write', publish)
        self.assertIn('gh workflow run publish-typescript-sdk.yml', publish)
        self.assertIn('--ref master', publish)
        self.assertIn('jcode_release_tag=$JCODE_BUILD_SEMVER', publish)
        self.assertNotIn('uses: ./.github/workflows/publish-typescript-sdk.yml', publish)

    def test_exact_source_ci_can_be_manually_retried_after_bot_push(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        triggers = workflow.split('\non:\n', 1)[1].split('\nconcurrency:', 1)[0]
        self.assertIn('\n  workflow_dispatch:\n', triggers)
        self.assertIn('\n  push:\n    branches: [main, master]', triggers)
        self.assertNotIn('inputs:', triggers)

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
