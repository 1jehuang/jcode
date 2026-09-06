#!/usr/bin/env python3
"""Built-CLI credential import acceptance with synthetic secrets and isolated homes.

Usage: python3 tests/test_auth_import_cli.py /absolute/path/to/jcode
No personal credentials, external provider requests, or SSH are used here.
"""
import json
import os
import pty
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

BINARY = str(Path(sys.argv.pop(1)).resolve()) if len(sys.argv) > 1 and not sys.argv[1].startswith('-') else None
SECRET = 'synthetic-import-cli-not-a-real-access-token'
REFRESH = 'synthetic-import-cli-not-a-real-refresh-token'


def envelope(provider='openai'):
    credential = ({'access_token': SECRET, 'refresh_token': REFRESH, 'expires_at': 4102444800000}
                  if provider == 'openai' else {'access': SECRET, 'refresh': REFRESH, 'expires': 4102444800000})
    return json.dumps({'version': 1, 'provider': provider, 'credential': credential}).encode()


@unittest.skipUnless(BINARY, 'supply a built CLI path')
class ImportCLI(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='jcode-import-cli-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / 'data'
        self.env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': str(self.root),
                    'JCODE_HOME': str(self.home), 'XDG_CONFIG_HOME': str(self.root / 'config'),
                    'JCODE_TELEMETRY': 'off', 'HTTPS_PROXY': 'http://127.0.0.1:9',
                    'HTTP_PROXY': 'http://127.0.0.1:9', 'ALL_PROXY': 'http://127.0.0.1:9', 'NO_PROXY': ''}

    def command(self, provider='openai'):
        return [BINARY, '--no-update', '--no-selfdev', 'auth', 'import', '--provider', provider, '--stdin', '--json']

    def run_import(self, payload, provider='openai'):
        result = subprocess.run(self.command(provider), input=payload, capture_output=True, env=self.env, timeout=10)
        for secret in (SECRET, REFRESH):
            self.assertNotIn(secret.encode(), result.stdout + result.stderr, 'secret exposed in CLI output')
        return result

    def test_import_both_providers_is_private_and_acknowledges_only_status(self):
        for provider, filename in [('openai', 'openai-auth.json'), ('claude', 'auth.json')]:
            result = self.run_import(envelope(provider), provider)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(json.loads(result.stdout), {'status': 'imported', 'provider': provider})
            store = self.home / filename
            self.assertIn(SECRET, store.read_text())
            self.assertEqual(store.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.home.stat().st_mode & 0o777, 0o700)
        self.assertEqual({p.name for p in self.home.iterdir()}, {'auth.json', 'openai-auth.json'})

    def test_existing_store_bytes_inode_and_mode_are_untouched(self):
        self.home.mkdir(mode=0o755)
        store = self.home / 'openai-auth.json'
        store.write_bytes(b'{malformed-existing-preserve-me')
        store.chmod(0o640)
        before = store.stat()
        result = self.run_import(envelope())
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout)['status'], 'error')
        self.assertEqual(store.read_bytes(), b'{malformed-existing-preserve-me')
        self.assertEqual((store.stat().st_ino, store.stat().st_mode, store.stat().st_mtime_ns),
                         (before.st_ino, before.st_mode, before.st_mtime_ns))
        self.assertEqual(self.home.stat().st_mode & 0o777, 0o755)
        self.assertEqual(list(self.home.iterdir()), [store])

    def test_invalid_oversized_and_wrong_provider_make_no_store(self):
        for payload in (b'', b'not json ' + SECRET.encode(), b'x' * 65537, envelope('claude')):
            result = self.run_import(payload)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(self.home.exists(), 'rejected import created storage')

    def test_unsupported_and_auto_provider_are_refused_before_stdin(self):
        for provider in ['auto', 'openai-api', 'bedrock']:
            process = subprocess.Popen(self.command(provider), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, env=self.env)
            try:
                process.wait(timeout=5)  # writer remains open, so this proves no input read
                self.assertNotEqual(process.returncode, 0)
            finally:
                if process.poll() is None:
                    process.kill()
                process.communicate()
            self.assertFalse(self.home.exists())

    def test_incremental_pipe_reads_wait_for_eof_then_import(self):
        process = subprocess.Popen(self.command(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env=self.env)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        payload = envelope()
        for chunk in [payload[:20], payload[20:]]:
            process.stdin.write(chunk)
            process.stdin.flush()
            time.sleep(0.03)
        self.assertIsNone(process.poll())
        process.stdin.close()
        process.stdin = None
        stdout, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 0)
        self.assertEqual(json.loads(stdout)['status'], 'imported')
        self.assertNotIn(SECRET.encode(), stdout + stderr)

    def test_simultaneous_imports_have_exactly_one_winner(self):
        processes = [subprocess.Popen(self.command(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, env=self.env) for _ in range(2)]
        try:
            for process in processes:
                process.stdin.write(envelope())
                process.stdin.close()
                process.stdin = None
            for process in processes:
                process.communicate(timeout=10)
            self.assertEqual(sorted(p.returncode == 0 for p in processes), [False, True])
            self.assertEqual({p.name for p in self.home.iterdir()}, {'openai-auth.json'})
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                    process.wait()

    def test_terminal_input_is_refused_without_prompting(self):
        master, slave = pty.openpty()
        try:
            process = subprocess.run(self.command(), stdin=slave, capture_output=True,
                                     env=self.env, timeout=5)
            self.assertNotEqual(process.returncode, 0)
            self.assertIn(b'piped stdin', process.stdout)
            self.assertFalse(self.home.exists())
        finally:
            os.close(master)
            os.close(slave)

    def test_invalid_arguments_and_help_do_not_modify_existing_credentials(self):
        self.home.mkdir(mode=0o755)
        store = self.home / 'openai-auth.json'
        store.write_bytes(b'existing-store-not-to-be-touched')
        store.chmod(0o644)
        before = store.stat()
        for args in [
            ['auth', 'import', '--provider', 'openai', '--json'],
            ['auth', 'import', '--provider', 'nonsense', '--stdin'],
            ['auth', 'import', '--help'],
            ['auth', 'import', '--stdin', '--overwrite'],
        ]:
            result = subprocess.run([BINARY, *args], input=b'', capture_output=True,
                                    env=self.env, timeout=5)
            self.assertEqual(result.returncode == 0, '--help' in args)
            self.assertEqual(store.read_bytes(), b'existing-store-not-to-be-touched')
            self.assertEqual((store.stat().st_mode, store.stat().st_mtime_ns),
                             (before.st_mode, before.st_mtime_ns))
            self.assertEqual(list(self.root.iterdir()), [self.home])
            self.assertEqual(list(self.home.iterdir()), [store])

    def test_stalled_writer_times_out_without_runtime_shutdown_hang(self):
        process = subprocess.Popen(self.command(), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env=self.env)
        try:
            process.wait(timeout=35)  # deliberately keep stdin open and silent
            stdout, _ = process.communicate()
            self.assertNotEqual(process.returncode, 0)
            self.assertIn(b'timed out', stdout)
            self.assertFalse(self.home.exists())
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()


if __name__ == '__main__':
    unittest.main(verbosity=2)
