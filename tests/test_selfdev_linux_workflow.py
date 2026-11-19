"""Exercise the Linux self-dev workflow's shell gates without publishing anything."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest

WORKFLOW = (Path(__file__).resolve().parents[1] / '.github/workflows/selfdev-linux.yml').read_text()
ARCHIVE = 'jcode-linux-x86_64.tar.gz'
VERIFY = 'Verify exact master commit'
INSPECT = 'Skip completed releases, resume interrupted drafts'
PUBLISH = 'Publish complete non-latest prerelease'


def script(name):
    body = WORKFLOW.split('name: ' + name + '\n', 1)[1].split('        run: |\n', 1)[1]
    lines = []
    for line in body.splitlines():
        if line and not line.startswith('          '):
            break
        lines.append(line)
    return textwrap.dedent('\n'.join(lines))


class LinuxSelfdevTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.git('init', '-qb', 'master')
        self.git('commit', '-qm', 'master', '--allow-empty')
        self.sha = self.git('rev-parse', 'HEAD')
        self.tag = 'selfdev-linux-' + self.sha
        self.env = {**os.environ, 'PATH': f'{self.root}:{os.environ["PATH"]}',
                    'GITHUB_OUTPUT': str(self.root / 'output'), 'GH_TOKEN': 'test',
                    'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_API_URL': 'https://invalid.example',
                    'GITHUB_SHA': self.sha, 'ARCHIVE': ARCHIVE, 'RUNNER_TEMP': str(self.root), 'SHA': self.sha}

    def git(self, *args):
        return subprocess.check_output(['git', '-c', 'user.name=test', '-c', 'user.email=t@example.com',
                                        *args], cwd=self.root, text=True).strip()

    def step(self, name, **env):
        return subprocess.run(['bash', '-c', script(name)], cwd=self.root, capture_output=True,
                              env={**self.env, **env}).returncode

    def mock(self, name, body):
        path = self.root / name
        path.write_text('#!/bin/bash\n' + body)
        path.chmod(0o755)

    def test_accepts_exact_master_commit_only(self):
        self.git('checkout', '-qb', 'side')
        self.git('commit', '-qm', 'unreachable', '--allow-empty')
        for invalid in [self.git('rev-parse', 'HEAD'), self.sha[:9], 'master', 'a' * 40, '--help']:
            self.assertNotEqual(self.step(VERIFY, SHA=invalid), 0)
        self.assertEqual(self.step(VERIFY), 0)
        self.git('tag', self.tag)
        self.assertNotEqual(self.step(VERIFY), 0)

    def test_inspection(self):
        self.mock('curl', 'echo "$STATUS"')
        self.git('tag', self.tag)
        assets = [{'name': name, 'size': 10, 'state': 'uploaded'} for name in [ARCHIVE, 'SHA256SUMS']]
        release = dict(tag_name=self.tag, target_commitish=self.sha, draft=False, prerelease=True, assets=assets)
        for status, changes, build in [('404', {}, True), ('403', {}, None), ('200', {}, False),
                                      ('200', {'assets': assets[:1]}, None), ('200', {'prerelease': False}, None),
                                      ('200', {'draft': True}, True), ('200', {'draft': True, 'target_commitish': 'master'}, None)]:
            with self.subTest(status=status, changes=changes):
                (self.root / 'release.json').write_text(json.dumps({**release, **changes}))
                output = self.root / 'output'
                output.unlink(missing_ok=True)
                result = self.step(INSPECT, STATUS=status)
                self.assertEqual(result == 0, build is not None)
                if build is not None:
                    self.assertEqual(output.read_text(), f'build={str(build).lower()}\n')

    def test_draft_retry_and_non_latest_publication(self):
        (self.root / ARCHIVE).write_bytes(b'test')
        (self.root / 'SHA256SUMS').write_bytes(subprocess.check_output(['sha256sum', ARCHIVE], cwd=self.root))
        self.mock('gh', '''echo "$*" >> calls
if [[ $2 == view ]]; then
  if [[ $MODE == new && $# == 3 ]]; then exit 1; fi
  cat status
fi
''')
        for mode in ['new', 'draft', 'published']:
            calls = self.root / 'calls'
            calls.write_text('')
            (self.root / 'status').write_text(json.dumps(dict(isDraft=mode != 'published', isPrerelease=True, targetCommitish=self.sha)))
            result = self.step(PUBLISH, MODE=mode)
            log = calls.read_text().splitlines()
            self.assertEqual(result == 0, mode != 'published')
            if mode == 'published':
                self.assertFalse(any('release upload' in line or 'release edit' in line for line in log))
                continue
            self.assertEqual(any('release create' in line for line in log), mode == 'new')
            self.assertIn('release upload', log[-2])
            self.assertIn('--draft=false --prerelease --latest=false', log[-1])


if __name__ == '__main__':
    unittest.main()
