"""End-to-end test for scripts/publish_linux_release_direct.sh using stubbed gh and hosted build."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO_ROOT = Path(__file__).resolve().parents[1]
PUBLISH_SCRIPT = REPO_ROOT / 'scripts/publish_linux_release_direct.sh'
HIGHLIGHTS_SCRIPT = REPO_ROOT / 'scripts/selfdev_release_highlights.py'


class PublishLinuxReleaseDirectTest(unittest.TestCase):
    def _setup_repo(self, root: Path):
        repo = root / 'repo'
        (repo / 'scripts').mkdir(parents=True)
        shutil.copy2(PUBLISH_SCRIPT, repo / 'scripts/publish_linux_release_direct.sh')
        shutil.copy2(HIGHLIGHTS_SCRIPT, repo / 'scripts/selfdev_release_highlights.py')
        (repo / 'Cargo.toml').write_text('[package]\nname = "jcode"\nversion = "1.1.0"\n', encoding='utf-8')

        # Stub hosted build script that writes a fake executable binary reporting the expected version.
        hosted_build = repo / 'scripts/build_preview_linux_hosted.sh'
        hosted_build.write_text('''#!/usr/bin/env bash
set -euo pipefail
sha=$(git rev-parse --short HEAD)
out="target/x86_64-unknown-linux-gnu/release/jcode"
mkdir -p "$(dirname "$out")"
cat > "$out" <<EOF
#!/usr/bin/env sh
echo "jcode ${JCODE_BUILD_SEMVER} (${sha})"
EOF
chmod 755 "$out"
''', encoding='utf-8')
        hosted_build.chmod(0o755)

        bin_dir = root / 'bin'
        bin_dir.mkdir()
        calls = root / 'gh_calls.log'
        gh = bin_dir / 'gh'
        gh.write_text('''#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$GH_CALLS"
if [ "$1" = "api" ]; then
  for arg in "$@"; do
    if [ "$arg" = "--arg" ]; then
      echo "accepts 1 arg(s), received extra --arg" >&2
      exit 1
    fi
  done
  if [ "${FAIL_API:-0}" = "1" ]; then
    echo "simulated gh api failure" >&2
    exit 1
  fi
  case "$*" in
    *git/ref/heads/master*)
      echo "$REMOTE_SHA"
      ;;
    *releases?per_page=100*--jq*)
      printf '2026-11-23T09:10:12Z\\tv1.1.2\\n'
      ;;
    *releases?per_page=100*)
      echo '[]'
      ;;
    *)
      echo '{}'
      ;;
  esac
  exit 0
fi

if [ "$1" = "release" ] && [ "$2" = "view" ]; then
  if [ -f "$RELEASE_CREATED" ]; then
    printf '{"isDraft":true,"isPrerelease":false,"targetCommitish":"%s","assets":[],"body":""}\\n' "$REMOTE_SHA"
    exit 0
  fi
  exit 1
fi

if [ "$1" = "release" ] && [ "$2" = "create" ]; then
  touch "$RELEASE_CREATED"
  exit 0
fi

if [ "$1" = "release" ] && { [ "$2" = "upload" ] || [ "$2" = "edit" ]; }; then
  exit 0
fi

if [ "$1" = "workflow" ] && [ "$2" = "run" ]; then
  if [ "${FAIL_DISCORD:-0}" = "1" ]; then
    echo "simulated discord dispatch failure" >&2
    exit 1
  fi
  exit 0
fi

exit 0
''', encoding='utf-8')
        gh.chmod(0o755)

        # Ensure docker is not invoked in unit tests.
        docker = bin_dir / 'docker'
        docker.write_text('#!/bin/sh\nexit 1\n', encoding='utf-8')
        docker.chmod(0o755)

        jcode_home = root / 'jcode_home'
        jcode_home.mkdir()
        env = dict(
            os.environ,
            PATH=f'{bin_dir}:{os.environ["PATH"]}',
            JCODE_HOME=str(jcode_home),
            JCODE_SCRATCH_DIR=str(root / 'scratch'),
            GH_CALLS=str(calls),
            RELEASE_CREATED=str(root / 'release_created'),
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_NOSYSTEM='1',
        )

        def git(*args):
            return subprocess.check_output(['git', *args], cwd=repo, env=env, text=True).strip()

        git('init', '-qb', 'master')
        git('config', 'user.name', 'Test')
        git('config', 'user.email', 'test@example.invalid')
        git('config', 'commit.gpgsign', 'false')
        git('config', 'jcode.linuxPrerelease', 'true')
        git('add', '.')
        git('commit', '-qm', 'initial')
        git('tag', 'v1.1.2')
        (repo / 'README.md').write_text('hello\n', encoding='utf-8')
        git('add', 'README.md')
        git('commit', '-qm', 'next release')
        sha = git('rev-parse', 'HEAD')
        env['REMOTE_SHA'] = sha
        return repo, sha, jcode_home, calls, env

    def test_happy_path_publishes_and_writes_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo, sha, jcode_home, calls, env = self._setup_repo(root)
            proc = subprocess.run(
                ['bash', 'scripts/publish_linux_release_direct.sh', sha, 'Test release notes'],
                cwd=repo,
                env=env,
                check=False,
            )
            self.assertEqual(proc.returncode, 0, (jcode_home / 'logs/linux-release.log').read_text())
            status = json.loads((jcode_home / 'logs/linux-release-status.json').read_text())
            self.assertEqual(status['sha'], sha)
            self.assertEqual(status['tag'], 'v1.1.3')
            self.assertEqual(status['state'], 'published')
            self.assertEqual(status['discord_status'], 'dispatched')
            gh_calls = calls.read_text()
            self.assertIn('workflow run discord-release.yml --repo 1jehuang/jcode --ref master -f tag=v1.1.3', gh_calls)

    def test_discord_dispatch_failure_marks_status_failed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo, sha, jcode_home, _calls, env = self._setup_repo(root)
            proc = subprocess.run(
                ['bash', 'scripts/publish_linux_release_direct.sh', sha, 'Test release notes'],
                cwd=repo,
                env={**env, 'FAIL_DISCORD': '1'},
                check=False,
            )
            self.assertNotEqual(proc.returncode, 0)
            status = json.loads((jcode_home / 'logs/linux-release-status.json').read_text())
            self.assertEqual(status['state'], 'failed')
            self.assertEqual(status['step'], 'dispatch_discord')
            self.assertEqual(status['discord_status'], 'failed')

    def test_api_failure_marks_status_failed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo, sha, jcode_home, _calls, env = self._setup_repo(root)
            proc = subprocess.run(
                ['bash', 'scripts/publish_linux_release_direct.sh', sha],
                cwd=repo,
                env={**env, 'FAIL_API': '1'},
                check=False,
            )
            self.assertNotEqual(proc.returncode, 0)
            status = json.loads((jcode_home / 'logs/linux-release-status.json').read_text())
            self.assertEqual(status['state'], 'failed')
            self.assertEqual(status['step'], 'check_remote_master')
            self.assertIn('simulated gh api failure', status['detail'])

    def test_direct_discord_webhook_skips_workflow_dispatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo, sha, jcode_home, calls, env = self._setup_repo(root)
            marker = root / 'discord_posted.txt'
            (repo / 'scripts/post_discord_release.py').write_text(
                f'import os, sys; open({str(marker)!r}, "w").write(" ".join(sys.argv[1:]) + "|" + os.environ["DISCORD_RELEASE_WEBHOOK"])\n',
                encoding='utf-8',
            )
            subprocess.check_call(['git', 'add', 'scripts/post_discord_release.py'], cwd=repo, env=env)
            subprocess.check_call(['git', 'commit', '-qm', 'add stub discord poster'], cwd=repo, env=env)
            sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, env=env, text=True).strip()
            env['REMOTE_SHA'] = sha
            (jcode_home / 'discord-release-webhook').write_text('https://discord.test/webhook\n', encoding='utf-8')
            proc = subprocess.run(
                ['bash', 'scripts/publish_linux_release_direct.sh', sha, 'Direct notes'],
                cwd=repo,
                env=env,
                check=False,
            )
            self.assertEqual(proc.returncode, 0, (jcode_home / 'logs/linux-release.log').read_text())
            status = json.loads((jcode_home / 'logs/linux-release-status.json').read_text())
            self.assertEqual(status['state'], 'published')
            self.assertEqual(status['discord_status'], 'posted')
            self.assertEqual(marker.read_text(), '--tag v1.1.3|https://discord.test/webhook')
            self.assertNotIn('workflow run discord-release.yml', calls.read_text())


if __name__ == '__main__':
    unittest.main()
