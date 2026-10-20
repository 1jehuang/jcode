"""Regression checks for release routing and the compatibility shell command.

The compatibility checks execute the actual selection fragment with stubbed
compiler/installer commands, not a real Docker build.
"""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PublicReleaseTests(unittest.TestCase):
    def test_workflow_preserves_release_and_toolchains(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn('CARGO_PROFILE_RELEASE_OPT_LEVEL: "3"', workflow)
        self.assertIn("JUSTRUST_TOOLCHAIN: system", workflow)
        self.assertIn("RUSTUP_TOOLCHAIN=nightly justrust build --local --locked -Z build-std=std,panic_abort", workflow)
        self.assertIn('& justrust @cargoArgs', workflow)
        self.assertIn('if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }', workflow)
        cargo = (ROOT / "Cargo.toml").read_text()
        self.assertIn('[profile.release]\nopt-level = 1', cargo)
        self.assertIn('[profile.selfdev]\ninherits = "release"\nopt-level = 0', cargo)

    def test_freebsd_lock_guard_without_git_metadata(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        vm = workflow.split('  build-freebsd:', 1)[1]
        fragment = vm[vm.index('            lock_snapshot='):]
        fragment = fragment[:fragment.index('            echo "::endgroup::"')]
        fragment = '\n'.join(line[12:] for line in fragment.splitlines())
        self.assertIn('cargo build --locked --release -p jcode --bin jcode', fragment)
        self.assertNotIn('git ', fragment)
        for mode in ('unchanged', 'modified', 'missing', 'missing_before_build',
                     'missing_snapshot', 'build_failed'):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                source = directory / 'source'
                source.mkdir()
                self.assertFalse((source / '.git').exists())
                if mode != 'missing_before_build':
                    (source / 'Cargo.lock').write_text('original lockfile\n')
                cargo = directory / 'cargo'
                cargo.write_text(
                    '#!/bin/sh\n'
                    'printf "%s\\n" "$@" > cargo-args\n'
                    # The snapshot must not be inside the copied source tree.
                    'case "$lock_snapshot" in "$PWD"/*) exit 90 ;; esac\n'
                    'printf "%s" "$lock_snapshot" > snapshot-path\n'
                    'case "$LOCK_TEST_MODE" in\n'
                    'modified) printf "changed\\n" > Cargo.lock ;;\n'
                    'missing) rm Cargo.lock ;;\n'
                    'missing_snapshot) rm "$lock_snapshot" ;;\n'
                    'build_failed) exit 42 ;;\n'
                    'esac\n'
                )
                cargo.chmod(0o755)
                env = dict(os.environ, PATH=f"{directory}:{os.environ['PATH']}",
                           LOCK_TEST_MODE=mode)
                result = subprocess.run(
                    ['/bin/sh', '-ec', 'export lock_snapshot\n' + fragment],
                    cwd=source, env=env, capture_output=True, text=True)
                if mode == 'unchanged':
                    self.assertEqual(result.returncode, 0, result.stderr)
                else:
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                if mode == 'missing_before_build':
                    self.assertFalse((source / 'cargo-args').exists())
                else:
                    self.assertEqual((source / 'cargo-args').read_text().splitlines(),
                                     ['build', '--locked', '--release', '-p', 'jcode',
                                      '--bin', 'jcode'])
                    snapshot = Path((source / 'snapshot-path').read_text())
                    self.assertFalse(snapshot.exists(), 'snapshot was not cleaned up')

    def test_compatibility_ci_and_local_selection(self):
        source = (ROOT / "scripts/build_linux_compat.sh").read_text()
        self.assertIn('-e CARGO_PROFILE_RELEASE_OPT_LEVEL', source)
        self.assertIn('-e CARGO_BUILD_JOBS', source)
        self.assertIn('-e JCODE_CI_BUILD', source)
        # Execute exactly the command-selection block passed to Docker.
        fragment = source[source.index('build_command=(cargo build)'):]
        fragment = fragment[:fragment.index('\n\n        # Exercise the exact portable binary')]
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for name in ('cargo', 'justrust', 'bash'):
                command = directory / name
                command.write_text(
                    '#!/bin/sh\n'
                    'printf "%s" "' + name + '"\n'
                    'printf " <%s>" "$@"\n'
                    'printf " toolchain=%s slots=%s level=%s\\n" '
                    '"${JUSTRUST_TOOLCHAIN:-}" "${JUSTRUST_SLOTS:-}" '
                    '"${CARGO_PROFILE_RELEASE_OPT_LEVEL:-}"\n'
                )
                command.chmod(0o755)
            for ci in ('0', '1'):
                with self.subTest(ci=ci):
                    # Stubbed commands must not be shadowed by a caller's
                    # exported self-dev Cargo function or BASH_ENV startup file.
                    env = {key: value for key, value in os.environ.items()
                           if not key.startswith('BASH_FUNC_') and key != 'BASH_ENV'}
                    env.update(PATH=f"{directory}:{os.environ['PATH']}",
                               JCODE_CI_BUILD=ci, JCODE_COMPAT_PROFILE='release',
                               JCODE_COMPAT_TARGET='x86_64-unknown-linux-gnu',
                               CARGO_PROFILE_RELEASE_OPT_LEVEL='3')
                    result = subprocess.run(['/bin/bash', '-eu', '-c', fragment],
                                            env=env, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn('<--locked>', result.stdout)
                    self.assertIn('<--profile> <release> <--target> <x86_64-unknown-linux-gnu>', result.stdout)
                    self.assertIn('<linux-compat-vendored-openssl>', result.stdout)
                    if ci == '1':
                        self.assertIn('bash <scripts/install_justrust_ci.sh>', result.stdout)
                        self.assertIn('justrust <build> <--local>', result.stdout)
                        self.assertIn('toolchain=system slots=0 level=3', result.stdout)
                    else:
                        self.assertIn('cargo <build>', result.stdout)
                        self.assertNotIn('install_justrust', result.stdout)
                        self.assertNotIn('justrust <build>', result.stdout)

    def test_cloud_artifact_gate_rejects_missing_protocol_and_newer_glibc(self):
        source = (ROOT / "scripts/build_linux_compat.sh").read_text()
        fragment = source[source.index('        built="$CARGO_TARGET_DIR/'):]
        fragment = fragment[:fragment.index('\n\n\t    cp ')]
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            binary = directory / 'target/x86_64-unknown-linux-gnu/release/jcode'
            binary.parent.mkdir(parents=True)
            binary.write_text(
                '#!/bin/sh\n'
                'printf "%s\\n" "$*" >> "$TEST_CALLS"\n'
                'case "$*" in *"cloud ${FAIL_CLOUD:-never} --help"*) exit 1 ;; esac\n'
            )
            binary.chmod(0o755)
            objdump = directory / 'objdump'
            objdump.write_text('#!/bin/sh\nprintf "0000 DF .text %s symbol\\n" "$TEST_GLIBC_SYMBOL"\n')
            objdump.chmod(0o755)
            for glibc, missing in [('2.17', ''), ('2.17-legacy', ''), ('2.34-legacy', ''), ('2.34', ''), ('2.17', 'receive'),
                                   ('2.17', 'credentials'), ('2.17', 'cancel-return')]:
                with self.subTest(glibc=glibc, missing=missing):
                    calls = directory / 'calls'
                    calls.write_text('')
                    env = dict(os.environ, PATH=f"{directory}:{os.environ['PATH']}",
                               CARGO_TARGET_DIR=str(directory / 'target'),
                               JCODE_COMPAT_TARGET='x86_64-unknown-linux-gnu',
                               JCODE_COMPAT_PROFILE='release',
                               TEST_GLIBC_SYMBOL=('GLIBC_' + glibc.removesuffix('-legacy')
                                                  if glibc.endswith('-legacy') else f'(GLIBC_{glibc})'),
                               FAIL_CLOUD=missing, TEST_CALLS=str(calls))
                    result = subprocess.run(['/bin/bash', '-euo', 'pipefail', '-c', fragment],
                                            env=env, capture_output=True, text=True)
                    if glibc.removesuffix('-legacy') == '2.17' and not missing:
                        self.assertEqual(result.returncode, 0, result.stderr)
                        for command in ['receive', 'credentials', 'cancel-return']:
                            self.assertIn(f'cloud {command} --help', calls.read_text())
                    else:
                        self.assertNotEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
