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
INSPECT = 'Resolve semver tag, skip completed releases, resume interrupted drafts'
PUBLISH = 'Publish Linux release'


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
        self.tag = 'v1.2.3'
        self.git('tag', self.tag)
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

    def test_inspection(self):
        self.mock('curl', 'echo "$STATUS"')
        assets = [{'name': name, 'size': 10, 'state': 'uploaded'} for name in [ARCHIVE, 'SHA256SUMS']]
        release = dict(tag_name=self.tag, target_commitish=self.sha, draft=False, prerelease=False, assets=assets)
        for status, changes, build in [('404', {}, True), ('403', {}, None), ('200', {}, False),
                                      ('200', {'assets': assets[:1]}, None), ('200', {'prerelease': True}, None),
                                      ('200', {'draft': True}, True), ('200', {'draft': True, 'target_commitish': 'master'}, None)]:
            with self.subTest(status=status, changes=changes):
                (self.root / 'release.json').write_text(json.dumps({**release, **changes}))
                output = self.root / 'output'
                output.unlink(missing_ok=True)
                result = self.step(INSPECT, STATUS=status)
                self.assertEqual(result == 0, build is not None)
                if build is not None:
                    self.assertEqual(output.read_text(), f'tag={self.tag}\nbuild={str(build).lower()}\n')

    def test_draft_retry_and_non_latest_publication(self):
        dist = self.root / 'dist'
        dist.mkdir()
        (self.root / 'scripts').symlink_to(Path(__file__).resolve().parents[1] / 'scripts')
        (dist / ARCHIVE).write_bytes(b'test')
        (dist / 'SHA256SUMS').write_bytes(subprocess.check_output(['sha256sum', ARCHIVE], cwd=dist))
        self.mock('gh', '''echo "$*" >> "$RUNNER_TEMP/calls"
case "$1 $2" in
  'release view')
    if [[ $5 == body ]]; then
      [[ $MODE != new ]] || exit 1
      cat "$RUNNER_TEMP/body"
    else cat "$RUNNER_TEMP/status"; fi ;;
  'release create'|'release edit')
    op=$2
    while [[ $# -gt 0 ]]; do
      if [[ $1 == --notes-file ]]; then cp "$2" "$RUNNER_TEMP/$op-notes"; exit; fi
      shift
    done
    exit 1 ;;
esac
''')
        for mode in ['new', 'draft', 'legacy-draft', 'published']:
            saved = 'Original description\n\n## Highlights\n\n- Original highlight\n'
            (self.root / 'body').write_text(saved if mode != 'legacy-draft' else 'Old description')
            calls = self.root / 'calls'
            calls.write_text('')
            (self.root / 'status').write_text(json.dumps(dict(isDraft=mode != 'published', isPrerelease=False, targetCommitish=self.sha)))
            result = self.step(PUBLISH, TAG=self.tag, MODE=mode, GH_REPO='owner/repo', RELEASE_NOTES='Fresh highlights')
            log = calls.read_text().splitlines()
            self.assertEqual(result == 0, mode != 'published')
            if mode == 'published':
                self.assertFalse(any('release upload' in line or 'release edit' in line for line in log))
                continue
            self.assertEqual(any('release create' in line for line in log), mode == 'new')
            published = (self.root / 'edit-notes').read_text()
            if mode == 'draft':
                self.assertEqual(published, saved)
            else:
                self.assertIn('## Highlights\n\n- Fresh highlights', published)
            if mode == 'new':
                self.assertEqual(published, (self.root / 'create-notes').read_text())
            self.assertIn('release upload', log[-2])
            self.assertIn('--draft=false --prerelease=false --latest', log[-1])


if __name__ == '__main__':
    unittest.main()
