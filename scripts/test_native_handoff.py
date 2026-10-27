#!/usr/bin/env python3
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import native_handoff as handoff

ROOT = Path(__file__).resolve().parents[1]


class HandoffTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.old_cwd = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self.old_cwd)
        self.env = patch.dict(os.environ, {
            'GITHUB_SHA': 'a' * 40, 'GITHUB_REPOSITORY': '1jehuang/jcode',
            'GITHUB_REF': 'refs/heads/master', 'GITHUB_RUN_ID': '123',
            'GITHUB_RUN_ATTEMPT': '1',
        })
        self.env.start()
        self.addCleanup(self.env.stop)
        Path('Cargo.lock').write_text('frozen dependencies')
        self.directory = Path('handoff')
        self.directory.mkdir()
        os.environ['JUSTRUST_HOME'] = str(self.root / 'justrust')
        meta_dir = self.root / 'justrust/runs/test-run'
        meta_dir.mkdir(parents=True)
        (meta_dir / 'meta.json').write_text(json.dumps({'place': 'hosted'}))

    def binary(self, lane='linux-arm64'):
        data = bytearray(64)
        if lane == 'linux-arm64':
            data[:6] = b'\x7fELF\x02\x01'
            struct.pack_into('<HH', data, 16, 2, 183)
        else:
            data[:4] = b'\xcf\xfa\xed\xfe'
            struct.pack_into('<I', data, 4, 0x0100000C)
            struct.pack_into('<I', data, 12, 2)
        (self.directory / 'jcode').write_bytes(data)
        (self.directory / 'build-report.json').write_text(json.dumps({
            'id': 'test-run', 'exit': 0,
            'args': [arg for arg in handoff.CONTRACTS[lane]['command']
                     if arg not in ('--remote', '--local')],
            'remote': {'machine': 'hosted'} if lane == 'linux-arm64' else None,
        }))

    def test_both_architectures_roundtrip(self):
        for lane in handoff.CONTRACTS:
            self.binary(lane)
            digest = handoff.seal(self.directory, lane)
            handoff.verify(self.directory, lane, digest)

    def test_binary_tampering_rejected(self):
        self.binary()
        digest = handoff.seal(self.directory, 'linux-arm64')
        with (self.directory / 'jcode').open('ab') as output:
            output.write(b'tampered')
        with self.assertRaisesRegex(ValueError, 'mismatch'):
            handoff.verify(self.directory, 'linux-arm64', digest)

    def test_each_context_field_is_bound(self):
        self.binary()
        digest = handoff.seal(self.directory, 'linux-arm64')
        for key in ('GITHUB_SHA', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT',
                    'GITHUB_REPOSITORY', 'GITHUB_REF'):
            value = 'b' * 40 if key == 'GITHUB_SHA' else 'other'
            with self.subTest(key=key), patch.dict(os.environ, {key: value}):
                with self.assertRaises(ValueError):
                    handoff.verify(self.directory, 'linux-arm64', digest)

    def test_manifest_tampering_rejected_even_with_new_digest(self):
        self.binary()
        handoff.seal(self.directory, 'linux-arm64')
        manifest = self.directory / 'provenance.json'
        for key in ('toolchain', 'justrust_revision', 'contract', 'features', 'schema'):
            original = manifest.read_text()
            data = json.loads(original)
            data[key] = 'untrusted'
            manifest.write_text(json.dumps(data))
            with self.subTest(key=key), self.assertRaises(ValueError):
                handoff.verify(self.directory, 'linux-arm64', handoff.digest(manifest))
            manifest.write_text(original)

    def test_wrong_manifest_digest_and_lock_rejected(self):
        self.binary()
        digest = handoff.seal(self.directory, 'linux-arm64')
        with self.assertRaisesRegex(ValueError, 'digest'):
            handoff.verify(self.directory, 'linux-arm64', '0' * 64)
        Path('Cargo.lock').write_text('changed')
        with self.assertRaisesRegex(ValueError, 'mismatch'):
            handoff.verify(self.directory, 'linux-arm64', digest)

    def test_wrong_architecture_truncation_and_symlink_rejected(self):
        self.binary('macos-arm64')
        with self.assertRaises(ValueError):
            handoff.seal(self.directory, 'linux-arm64')
        (self.directory / 'jcode').write_bytes(b'\x7fELF')
        with self.assertRaises(ValueError):
            handoff.seal(self.directory, 'linux-arm64')
        (self.directory / 'jcode').unlink()
        (self.directory / 'jcode').symlink_to('../Cargo.lock')
        with self.assertRaisesRegex(ValueError, 'regular'):
            handoff.seal(self.directory, 'linux-arm64')

    def test_manifest_symlink_rejected(self):
        self.binary()
        digest = handoff.seal(self.directory, 'linux-arm64')
        manifest = self.directory / 'provenance.json'
        manifest.rename('elsewhere.json')
        manifest.symlink_to('../elsewhere.json')
        with self.assertRaisesRegex(ValueError, 'regular'):
            handoff.verify(self.directory, 'linux-arm64', digest)

    def test_smoke_subprocess_environment_and_timeout(self):
        self.binary()
        result = subprocess.CompletedProcess([], 0, stdout='jcode Usage:', stderr='')
        with patch('native_handoff.subprocess.run', return_value=result) as run:
            handoff.smoke(self.directory)
        self.assertEqual([call.args[0][1:] for call in run.call_args_list],
                         [['--version'], ['--help'], ['run', '--help']])
        for call in run.call_args_list:
            self.assertEqual(call.kwargs['timeout'], 30)
            self.assertTrue(call.kwargs['check'])
            self.assertNotIn('GITHUB_TOKEN', call.kwargs['env'])
            self.assertNotIn('JCODE_API_KEY', call.kwargs['env'])

    def test_smoke_executes_real_credential_free_fixture(self):
        binary = self.directory / 'jcode'
        binary.write_text('#!/bin/sh\n'
                          '[ -z "${JCODE_API_KEY:-}" ] || exit 99\n'
                          'case "$1" in\n'
                          '  --version) echo "jcode fixture" ;;\n'
                          '  --help|run) echo "Usage: fixture" ;;\n'
                          '  *) exit 98 ;;\n'
                          'esac\n')
        with patch.dict(os.environ, {'JCODE_API_KEY': 'must-not-leak'}):
            handoff.smoke(self.directory)

    def test_smoke_rejects_bad_output_and_failure(self):
        self.binary()
        with patch('native_handoff.subprocess.run', return_value=
                   subprocess.CompletedProcess([], 0, stdout='', stderr='')):
            with self.assertRaises(ValueError):
                handoff.smoke(self.directory)
        with patch('native_handoff.subprocess.run', side_effect=subprocess.TimeoutExpired('jcode', 30)):
            with self.assertRaises(subprocess.TimeoutExpired):
                handoff.smoke(self.directory)

    def test_wrong_build_receipt_rejected(self):
        for field, value in [('exit', 1), ('args', ['build']), ('remote', None)]:
            self.binary()
            report = self.directory / 'build-report.json'
            data = json.loads(report.read_text())
            data[field] = value
            report.write_text(json.dumps(data))
            with self.subTest(field=field), self.assertRaises(ValueError):
                handoff.seal(self.directory, 'linux-arm64')
        self.binary()
        (self.root / 'justrust/runs/test-run/meta.json').write_text(json.dumps({'place': 'remote'}))
        with self.assertRaisesRegex(ValueError, 'hosted'):
            handoff.seal(self.directory, 'linux-arm64')

    def test_build_report_tampering_rejected(self):
        self.binary()
        digest = handoff.seal(self.directory, 'linux-arm64')
        report = self.directory / 'build-report.json'
        report.write_text(report.read_text() + '\n')
        with self.assertRaisesRegex(ValueError, 'mismatch'):
            handoff.verify(self.directory, 'linux-arm64', digest)

    def test_workflow_keeps_contract_and_trust_boundaries(self):
        workflow = (ROOT / '.github/workflows/native-handoff.yml').read_text()
        self.assertIn(handoff.TOOLCHAIN, workflow)
        self.assertIn(handoff.JUSTRUST_REV, workflow)
        for contract in handoff.CONTRACTS.values():
            self.assertIn('justrust ' + ' '.join(contract['command']), workflow)
        self.assertIn('artifact-ids: ${{ needs.producer.outputs.artifact_id }}', workflow)
        self.assertIn('environment: native-handoff-signing', workflow)
        self.assertIn("github.ref == 'refs/heads/master'", workflow)
        self.assertNotIn('\n  workflow_run:', workflow)
        self.assertNotIn('id-token: write', workflow)
        self.assertNotIn('contents: write', workflow)
        self.assertNotIn('gh release', workflow)

    def test_signing_missing_credentials_fails_without_tools(self):
        credentials = {name: 'fixture' for name in (
            'APPLE_CERTIFICATE_P12_BASE64', 'APPLE_CERTIFICATE_PASSWORD',
            'APPLE_SIGNING_IDENTITY', 'APPLE_NOTARY_KEY_P8_BASE64',
            'APPLE_NOTARY_KEY_ID', 'APPLE_NOTARY_ISSUER')}
        for missing in credentials:
            env = {'PATH': os.defpath, **credentials}
            del env[missing]
            result = subprocess.run(['bash', str(ROOT / 'scripts/sign_native_handoff_macos.sh')],
                                    env=env, capture_output=True, text=True)
            with self.subTest(missing=missing):
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Required signing credential missing: ' + missing, result.stderr)


if __name__ == '__main__':
    unittest.main()
