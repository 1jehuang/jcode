#!/usr/bin/env python3
"""Signing and read-only preflight fixtures. Never contacts Apple or signs real code."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import native_handoff_preflight as preflight

ROOT = Path(__file__).resolve().parents[1]
IDENTITY = 'Developer ID Application: Fixture (ABCDEFGHIJ)'


class SigningTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        tools = self.root / 'tools'
        tools.mkdir()
        self.log = self.root / 'calls.jsonl'
        tool = tools / 'fake-tool'
        tool.write_text('#!' + sys.executable + '\n' + '''import json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with Path(os.environ['TEST_LOG']).open('a') as log:
    log.write(json.dumps([name, args]) + '\\n')
if name == 'uname':
    print('Darwin')
elif name == 'security' and args[0] == 'find-identity':
    identity = os.environ['APPLE_SIGNING_IDENTITY']
    if os.environ.get('TEST_WRONG_IDENTITY'):
        identity += ' wrong'
    print('  1) ' + 'A' * 40 + ' "' + identity + '"')
    if os.environ.get('TEST_DUPLICATE_IDENTITY'):
        print('  2) ' + 'B' * 40 + ' "' + identity + '"')
elif name == 'codesign' and '--verify' in args and os.environ.get('TEST_BAD_CHAIN'):
    sys.exit(1)
elif name == 'ditto':
    Path(args[-1]).write_bytes(b'fixture zip')
elif name == 'xcrun':
    print(json.dumps({'status': 'Accepted', 'id': 'fixture-submission'}))
''')
        tool.chmod(0o755)
        for name in ('uname', 'security', 'codesign', 'ditto', 'xcrun'):
            (tools / name).symlink_to(tool)
        (self.root / 'scripts').symlink_to(ROOT / 'scripts', target_is_directory=True)
        handoff = self.root / 'handoff'
        handoff.mkdir()
        (handoff / 'jcode').write_text('#!/bin/sh\n'
                                      '[ -z "${APPLE_CERTIFICATE_PASSWORD:-}" ] || exit 99\n'
                                      'echo "jcode Usage: fixture"\n')
        (handoff / 'provenance.json').write_text('{}')
        self.env = {'PATH': str(tools) + os.pathsep + os.defpath,
                    'HOME': str(self.root), 'TEST_LOG': str(self.log),
                    'GITHUB_REPOSITORY': '1jehuang/jcode', 'GITHUB_REF': 'refs/heads/master',
                    **{name: 'eA==' for name in preflight.SIGNING_SECRETS},
                    'APPLE_SIGNING_IDENTITY': IDENTITY}

    def run_script(self, **env):
        return subprocess.run(['bash', str(ROOT / 'scripts/sign_native_handoff_macos.sh')],
                              cwd=self.root, env={**self.env, **env}, capture_output=True,
                              text=True, timeout=30)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_success_checks_chain_identifier_team_and_signed_runtime(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        signing = next(args for name, args in calls if name == 'codesign' and '--sign' in args)
        self.assertEqual(signing[signing.index('--sign') + 1], 'A' * 40)
        verification = next(args for name, args in calls if name == 'codesign' and '--verify' in args)
        requirement = verification[verification.index('-R') + 1]
        for clause in ('anchor apple generic', 'certificate 1[field.1.2.840.113635.100.6.2.6] exists',
                       'certificate leaf[field.1.2.840.113635.100.6.1.13] exists',
                       'identifier "com.jcode.cli"', 'certificate leaf[subject.OU] = "ABCDEFGHIJ"'):
            self.assertIn(clause, requirement)
        self.assertEqual(result.stdout.count('jcode Usage: fixture'), 3)
        self.assertTrue((self.root / 'signed-handoff/signing-provenance.json').is_file())
        self.assertTrue(any(name == 'security' and args[0] == 'delete-keychain' for name, args in calls))

    def test_adhoc_partial_and_other_certificate_names_rejected(self):
        for identity in ('-', 'Fixture', 'A' * 40, 'Apple Development: Fixture (ABCDEFGHIJ)',
                         'Developer ID Application: Fixture (BAD)', IDENTITY + '\n'):
            with self.subTest(identity=identity):
                result = self.run_script(APPLE_SIGNING_IDENTITY=identity)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('exact Developer ID Application name', result.stderr)
                self.assertEqual(self.calls(), [])

    def test_ambiguous_or_nonexact_keychain_identity_fails_before_codesign(self):
        for flag in ('TEST_WRONG_IDENTITY', 'TEST_DUPLICATE_IDENTITY'):
            self.log.unlink(missing_ok=True)
            result = self.run_script(**{flag: '1'})
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('exactly one matching', result.stderr)
            self.assertFalse(any(name in ('codesign', 'xcrun') for name, _ in self.calls()))

    def test_bad_chain_fails_before_notarization(self):
        result = self.run_script(TEST_BAD_CHAIN='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(name == 'xcrun' for name, _ in self.calls()))

    def test_signed_runtime_failure_prevents_notarization(self):
        (self.root / 'handoff/jcode').write_text('#!/bin/sh\nexit 1\n')
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(name == 'xcrun' for name, _ in self.calls()))


class PreflightTests(unittest.TestCase):
    def environment(self):
        return {'protection_rules': [{'type': 'required_reviewers', 'reviewers': [{'type': 'User'}],
                                      'prevent_self_review': True}],
                'deployment_branch_policy': {'custom_branch_policies': True, 'protected_branches': False}}

    def test_protected_environment_with_all_names_is_ready(self):
        result = preflight.assess(self.environment(), [{'name': 'master', 'type': 'branch'}],
                                  preflight.SIGNING_SECRETS)
        self.assertTrue(result['ready'])
        self.assertTrue(result['prevent_self_review'])

    def test_missing_secret_reviewers_or_broad_branch_policy_not_ready(self):
        environment = self.environment()
        policy = [{'name': 'master', 'type': 'branch'}]
        missing = preflight.assess(environment, policy, [])
        self.assertFalse(missing['ready'])
        self.assertEqual(set(missing['missing_environment_secret_names']), set(preflight.SIGNING_SECRETS))
        environment['protection_rules'] = []
        self.assertFalse(preflight.assess(environment, policy, preflight.SIGNING_SECRETS)['ready'])
        for bad in ([{'name': '*', 'type': 'branch'}], [{'name': 'master', 'type': 'tag'}],
                    policy + [{'name': 'release', 'type': 'branch'}]):
            self.assertFalse(preflight.assess(self.environment(), bad, preflight.SIGNING_SECRETS)['ready'])

    def test_api_calls_are_get_only_and_do_not_print_values(self):
        response = subprocess.CompletedProcess([], 0, stdout='[{"secrets":[{"name":"TEST"}]}]', stderr='')
        with patch('native_handoff_preflight.subprocess.run', return_value=response) as run:
            self.assertEqual(preflight.names('repos/1jehuang/jcode/actions/secrets'), {'TEST'})
        self.assertEqual(run.call_args.args[0][:6], ['gh', 'api', '--method', 'GET', '--paginate', '--slurp'])
        self.assertTrue(run.call_args.kwargs['capture_output'])

    def test_api_errors_fail_closed_without_echoing_response_body(self):
        responses = [subprocess.CompletedProcess([], 1, stdout='', stderr='403 private diagnostic'),
                     subprocess.CompletedProcess([], 0, stdout='private invalid JSON', stderr='')]
        for response in responses:
            with patch('native_handoff_preflight.subprocess.run', return_value=response):
                with self.assertRaises(LookupError) as error:
                    preflight.github('repos/1jehuang/jcode/environments/native-handoff-signing')
                self.assertNotIn('private', str(error.exception))
        with patch('native_handoff_preflight.subprocess.run', side_effect=FileNotFoundError):
            with self.assertRaises(LookupError):
                preflight.github('repos/1jehuang/jcode/environments/native-handoff-signing')

    def test_missing_or_unreadable_environment_fails_closed(self):
        with patch('native_handoff_preflight.names', return_value=set()), \
                patch('native_handoff_preflight.github', side_effect=LookupError('not found or not visible')):
            result = preflight.inspect()
        self.assertFalse(result['signing']['ready'])
        self.assertFalse(result['hosted_arm_key_configured'])
        self.assertEqual(set(result['signing']['required_environment_secret_names']), set(preflight.SIGNING_SECRETS))


if __name__ == '__main__':
    unittest.main()
