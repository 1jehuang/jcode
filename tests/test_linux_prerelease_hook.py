"""Exercise the embedded build hook against real git and an offline gh stub."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HOOK = Path(__file__).resolve().parents[1] / 'crates/jcode-build-support/src/linux_prerelease.sh'


class LinuxPrereleaseHook(unittest.TestCase):
    def test_dispatch_guards(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / 'repo'
            repo.mkdir()
            calls = root / 'calls'
            gh = root / 'gh'
            gh.write_text('''#!/bin/sh
if [ "$1" = api ]; then
  [ -z "$FAIL_API" ] || exit 1
  [ -z "$CHANGE_SOURCE" ] || touch raced
  echo "$REMOTE"
else
  echo "$*" >> "$CALLS"
fi
''')
            gh.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', CALLS=str(calls),
                       GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1')

            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, env=env, text=True).strip()

            git('init', '-qb', 'master')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                'commit', '-qm', 'test', '--allow-empty')
            sha = git('rev-parse', 'HEAD')

            def run(**overrides):
                calls.unlink(missing_ok=True)
                subprocess.run(['sh', HOOK, sha], cwd=repo,
                               env={**env, 'REMOTE': sha, **overrides}, check=False)
                return calls.read_text().strip() if calls.exists() else None

            self.assertIsNone(run())
            git('config', 'jcode.linuxPrerelease', 'true')
            self.assertEqual(run(), f'workflow run selfdev-linux.yml --repo 1jehuang/jcode --ref master -f sha={sha}')
            self.assertIsNone(run(REMOTE='0' * 40))
            self.assertIsNone(run(FAIL_API='1'))
            self.assertIsNone(run(CHANGE_SOURCE='1'))
            self.assertIsNone(run())  # untracked file
            (repo / 'raced').unlink()
            git('switch', '-qc', 'feature')
            self.assertIsNone(run())
            git('checkout', '--detach', '-q')
            self.assertIsNone(run())


if __name__ == '__main__':
    unittest.main()
