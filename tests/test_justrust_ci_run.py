"""Unit tests for scripts/justrust_ci_run.sh."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/justrust_ci_run.sh'


class JustrustCiRunTest(unittest.TestCase):
    def _make_stub(self, directory: Path, name: str, out_file: Path):
        stub = directory / name
        stub.write_text(
            f'#!/bin/sh\nprintf "%s:%s\\n" "{name}" "$*" >> "{out_file}"\n',
            encoding='utf-8',
        )
        stub.chmod(0o755)

    def test_uses_hosted_justrust_when_api_key_present(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            out = root / 'calls.log'
            self._make_stub(root, 'justrust', out)
            self._make_stub(root, 'cargo', out)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', JCODE_API_KEY='test-key')
            subprocess.check_call(
                ['bash', str(SCRIPT), 'clippy', '--locked', '--all-targets', '--', '-D', 'warnings'],
                env=env,
            )
            self.assertEqual(
                out.read_text().strip(),
                'justrust:clippy --remote --locked --all-targets -- -D warnings',
            )

    def test_falls_back_to_cargo_without_api_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            out = root / 'calls.log'
            self._make_stub(root, 'justrust', out)
            self._make_stub(root, 'cargo', out)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}')
            env.pop('JCODE_API_KEY', None)
            subprocess.check_call(
                ['bash', str(SCRIPT), 'check', '--locked', '--all-targets'],
                env=env,
            )
            self.assertEqual(
                out.read_text().strip(),
                'cargo:check --locked --all-targets',
            )

    def test_disable_env_forces_cargo_fallback(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            out = root / 'calls.log'
            self._make_stub(root, 'justrust', out)
            self._make_stub(root, 'cargo', out)
            env = dict(
                os.environ,
                PATH=f'{root}:{os.environ["PATH"]}',
                JCODE_API_KEY='test-key',
                JUSTRUST_CI_DISABLE='1',
            )
            subprocess.check_call(
                ['bash', str(SCRIPT), 'test', '--locked', '-p', 'jcode-sdk'],
                env=env,
            )
            self.assertEqual(
                out.read_text().strip(),
                'cargo:test --locked -p jcode-sdk',
            )

    def test_routes_macos_target_to_hosted_macos_platform(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            out = root / 'calls.log'
            self._make_stub(root, 'justrust', out)
            self._make_stub(root, 'cargo', out)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', JCODE_API_KEY='test-key')
            subprocess.check_call(
                ['bash', str(SCRIPT), 'build', '--locked', '--release', '--target', 'aarch64-apple-darwin'],
                env=env,
            )
            self.assertEqual(
                out.read_text().strip(),
                'justrust:build --remote --platform=macos --locked --release --target aarch64-apple-darwin',
            )


if __name__ == '__main__':
    unittest.main()
