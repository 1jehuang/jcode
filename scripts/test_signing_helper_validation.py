"""Credential-free fixtures for validation guards and embedded Python contracts.

These do not claim native signing coverage. That requires the protected workflow.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/check_signing_helper_macos.sh'
TEXT = SCRIPT.read_text()
BLOCKS = re.findall(r"<<'PY'[^\n]*\n(.*?)\nPY", TEXT, re.S)


class ValidationFixtures(unittest.TestCase):
    def run_python(self, block, *args, env=None):
        return subprocess.run(['python3', '-c', BLOCKS[block], *map(str, args)],
                              env={**os.environ, **(env or {})}, text=True,
                              capture_output=True, check=False)

    def test_exact_identity_and_team(self):
        result = self.run_python(0, env={'APPLE_SIGNING_IDENTITY':
                                        'Developer ID Application: Example (ABCDEFGHIJ)'})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'ABCDEFGHIJ')

    def test_identity_refusals(self):
        for identity in ['-', 'Example', 'Developer ID Application: Example (SHORT)',
                         'Developer ID Application: Example (ABCDEFGHIJ)\n']:
            with self.subTest(identity=identity):
                self.assertNotEqual(self.run_python(0, env={
                    'APPLE_SIGNING_IDENTITY': identity}).returncode, 0)

    def test_fingerprint_exact_match_and_ambiguity(self):
        identity = 'Developer ID Application: Example (ABCDEFGHIJ)'
        fingerprint = 'A' * 40
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, 'identities')
            line = f'  1) {fingerprint} "{identity}"\n'
            for contents, success in [(line, True), (line * 2, False),
                                      (line.replace('Example', 'Example Extra'), False),
                                      ('0 valid identities found', False)]:
                path.write_text(contents)
                result = self.run_python(3, path, env={'APPLE_SIGNING_IDENTITY': identity})
                self.assertEqual(result.returncode == 0, success, result.stderr)
                if success:
                    self.assertEqual(result.stdout.strip(), fingerprint)

    def test_real_absolute_plan_contract(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, 'plan.json')
            plan = {'mode': 'plan-only', 'commands': [
                ['/usr/bin/codesign', '--sign', 'A' * 40],
                ['/usr/bin/codesign', '-R', '=anchor apple generic', 'artifact']]}
            path.write_text(json.dumps(plan))
            self.assertEqual(self.run_python(4, path).returncode, 0)
            for change in ['relative-tool', 'requirement-file', 'executed-mode']:
                invalid = json.loads(json.dumps(plan))
                if change == 'relative-tool':
                    invalid['commands'][0][0] = 'codesign'
                elif change == 'requirement-file':
                    invalid['commands'][1][-2] = 'anchor apple generic'
                else:
                    invalid['mode'] = 'signed-and-native-verified'
                path.write_text(json.dumps(invalid))
                self.assertNotEqual(self.run_python(4, path).returncode, 0)

    def test_receipt_public_allowlist(self):
        with tempfile.TemporaryDirectory() as tmp:
            execution = Path(tmp, 'execution.json')
            execution.write_text('{"mode":"signed-and-native-verified"}')
            env = dict(GITHUB_REPOSITORY='1jehuang/jcode', GITHUB_SHA='a' * 40,
                       GITHUB_REF='refs/heads/master', GITHUB_RUN_ID='123',
                       GITHUB_RUN_ATTEMPT='2', GITHUB_WORKFLOW='validation',
                       TRUSTED_SHA256='b' * 64, APPLE_CERTIFICATE_PASSWORD='DO-NOT-RECORD')
            result = self.run_python(5, tmp, 'c' * 40, 'd' * 64, 'ABCDEFGHIJ', execution, env=env)
            self.assertEqual(result.returncode, 0, result.stderr)
            raw = Path(tmp, 'receipt.json').read_text()
            receipt = json.loads(raw)
            self.assertNotIn('DO-NOT-RECORD', raw)
            self.assertEqual(receipt['run_attempt'], '2')
            self.assertEqual(receipt['unsigned_sha256'], 'b' * 64)
            self.assertFalse(receipt['notarized'])
            execution.write_text('{"mode":"plan-only"}')
            self.assertNotEqual(self.run_python(5, tmp, 'c' * 40, 'd' * 64,
                                               'ABCDEFGHIJ', execution, env=env).returncode, 0)

    def test_reject_untrusted_dispatch_before_credentials(self):
        for repo, ref, event in [('fork/jcode', 'refs/heads/master', 'workflow_dispatch'),
                                 ('1jehuang/jcode', 'refs/heads/topic', 'workflow_dispatch'),
                                 ('1jehuang/jcode', 'refs/heads/master', 'push')]:
            env = {**os.environ, 'GITHUB_REPOSITORY': repo, 'GITHUB_REF': ref,
                   'GITHUB_EVENT_NAME': event}
            result = subprocess.run(['bash', str(SCRIPT), 'sign'], env=env,
                                    capture_output=True, text=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Requires canonical master dispatch', result.stderr)

    def test_workflow_scope(self):
        workflow = (ROOT / '.github/workflows/signing-helper-validation.yml').read_text()
        self.assertIn('environment: native-handoff-signing', workflow)
        self.assertIn('ref: 17c344f2e83f836598b82715950497c0879aa713', workflow)
        self.assertNotIn('inputs:', workflow)
        self.assertNotIn('download-artifact', workflow)
        self.assertEqual(workflow.count('${{ secrets.'), 3)
        self.assertIn('path: signing-helper-validation/receipt.json', workflow)
        self.assertIn('-R "=$requirement"', TEXT)
        self.assertLess(TEXT.index('keychains.pending'), TEXT.index('security create-keychain'))


if __name__ == '__main__':
    unittest.main()
