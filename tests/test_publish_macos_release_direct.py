"""End-to-end test for scripts/publish_macos_release_direct.sh using stubbed gh and justrust."""
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest

REPO_ROOT = Path(__file__).resolve().parents[1]
PUBLISH_SCRIPT = REPO_ROOT / 'scripts/publish_macos_release_direct.sh'


class PublishMacosReleaseDirectTest(unittest.TestCase):
    def test_happy_path_builds_verifies_macho_and_uploads(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / 'repo'
            (repo / 'scripts').mkdir(parents=True)
            shutil.copy2(PUBLISH_SCRIPT, repo / 'scripts/publish_macos_release_direct.sh')
            (repo / 'README.md').write_text('hello\n', encoding='utf-8')

            bin_dir = root / 'bin'
            bin_dir.mkdir()
            calls = root / 'calls.log'

            # Stub justrust that writes a minimal valid 64-bit ARM64 Mach-O header.
            justrust = bin_dir / 'justrust'
            justrust.write_text('''#!/usr/bin/env bash
set -euo pipefail
echo "justrust:$*" >> "$CALLS"
if [ "$1" = "build" ]; then
  mkdir -p target/aarch64-apple-darwin/release
  python3 -c 'import struct, pathlib; pathlib.Path("target/aarch64-apple-darwin/release/jcode").write_bytes(struct.pack("<III", 0xFEEDFACF, 0x0100000C, 0) + b"\\x00"*64)'
fi
''', encoding='utf-8')
            justrust.chmod(0o755)

            gh = bin_dir / 'gh'
            gh.write_text('''#!/usr/bin/env bash
set -euo pipefail
echo "gh:$*" >> "$CALLS"
if [ "$1" = "release" ] && [ "$2" = "view" ]; then
  echo '{"assets":[]}'
  exit 0
fi
exit 0
''', encoding='utf-8')
            gh.chmod(0o755)

            jcode_home = root / 'jcode_home'
            jcode_home.mkdir()
            env = dict(
                os.environ,
                PATH=f'{bin_dir}:{os.environ["PATH"]}',
                JCODE_HOME=str(jcode_home),
                JCODE_SCRATCH_DIR=str(root / 'scratch'),
                CALLS=str(calls),
                GIT_CONFIG_GLOBAL=os.devnull,
                GIT_CONFIG_NOSYSTEM='1',
            )

            def git(*args):
                return subprocess.check_output(['git', *args], cwd=repo, env=env, text=True).strip()

            git('init', '-qb', 'master')
            git('config', 'user.name', 'Test')
            git('config', 'user.email', 'test@example.invalid')
            git('config', 'commit.gpgsign', 'false')
            git('add', '.')
            git('commit', '-qm', 'initial')
            git('tag', 'v1.1.3')
            sha = git('rev-parse', 'HEAD')

            proc = subprocess.run(
                ['bash', 'scripts/publish_macos_release_direct.sh', sha],
                cwd=repo,
                env=env,
                check=False,
            )
            self.assertEqual(proc.returncode, 0, (jcode_home / 'logs/macos-release.log').read_text())
            status = json.loads((jcode_home / 'logs/macos-release-status.json').read_text())
            self.assertEqual(status['state'], 'published')
            self.assertEqual(status['tag'], 'v1.1.3')
            log = calls.read_text()
            self.assertIn('justrust:build --remote --platform=macos --locked --release --target aarch64-apple-darwin -p jcode --bin jcode', log)
            self.assertIn('gh:release upload v1.1.3', log)
            self.assertIn('jcode-macos-aarch64.tar.gz', log)


if __name__ == '__main__':
    unittest.main()
