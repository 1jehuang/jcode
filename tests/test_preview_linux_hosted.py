"""Offline orchestration/ABI regression tests, not hosted acceptance evidence.

Run: python3 -m unittest discover -s tests -p test_preview_linux_hosted.py
Real hosted builds must separately use tenant authentication and normal admission.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/build_preview_linux_hosted.sh"
X86 = "x86_64-unknown-linux-gnu"
ARM = "aarch64-unknown-linux-gnu"


class HostedLinuxPreviewTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="jcode-hosted-preview-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        shutil.copy2(SCRIPT, self.root / "scripts" / SCRIPT.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("CARGO_", "JUSTRUST_", "JCODE_", "RUST"))}
        self.env.update(PATH=f"{self.bin}:{os.environ['PATH']}",
                        JCODE_BUILD_SEMVER="v1.0.0-preview.42", RECORD=str(self.root / "record"))
        self.command("git", "init", "-q")
        (self.root / ".gitignore").write_text("/bin/\n/record\n/target/\n")
        # Fixture repository identity only, never used for the source checkout.
        self.command("git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                     "add", "scripts", ".gitignore")
        self.command("git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                     "commit", "-qm", "fixture source")
        self.install("justrust", r'''#!/usr/bin/env python3
import json, os, pathlib, struct, subprocess, sys
with open(os.environ['RECORD'], 'a') as record:
    record.write(json.dumps({'args': sys.argv[1:], 'env': dict(os.environ)}) + '\n')
if sys.argv[1:] == ['remote', 'use', 'hosted']:
    sys.exit(int(os.environ.get('BACKEND_FAILURE', '0')))
# Model pinned source_roots(): caller cargo inherits RUSTUP_TOOLCHAIN, not
# JUSTRUST_TOOLCHAIN, and metadata probes target flags before hosted dispatch.
if subprocess.run(['cargo', 'metadata', '--format-version', '1', '--offline']).returncode:
    sys.exit(29)
if os.environ.get('BUILD_FAILURE'):
    sys.exit(23)
if os.environ.get('NO_ARTIFACT'):
    sys.exit(0)
target = sys.argv[sys.argv.index('--target') + 1].removesuffix('.2.17')
artifact = pathlib.Path('target') / target / 'release/jcode'
artifact.parent.mkdir(parents=True, exist_ok=True)
header = bytearray(64)
header[:7] = b'\x7fELF\x02\x01\x01'
machine = 183 if target.startswith('aarch64') else 62
struct.pack_into('<HH', header, 16, 3, int(os.environ.get('MACHINE', machine)))
artifact.write_bytes(b'not ELF' if os.environ.get('INVALID_ELF') else header)
artifact.chmod(0o755)
''')
        self.install("rustup", r'''#!/usr/bin/env python3
import json, os, pathlib, sys
path = pathlib.Path(os.environ['RECORD'] + '.rustup')
path.write_text(json.dumps(sys.argv[1:]))
sys.exit(int(os.environ.get('RUSTUP_FAILURE', '0')))
''')
        self.install("cargo", r'''#!/usr/bin/env python3
import os, pathlib, sys
assert sys.argv[1:] == ['metadata', '--format-version', '1', '--offline']
# Model the native ARM CI caller: metadata probes its host with ARM target
# flags. CI rustup default is stable even when the managed compiler is nightly.
selected = os.environ.get('RUSTUP_TOOLCHAIN', '1.99.0')
flags = os.environ.get('CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS', '')
if '-Ztls-model=emulated' in flags:
    if selected != 'nightly-2026-10-08':
        print('error: the option Z is only accepted on the nightly compiler', file=sys.stderr)
        sys.exit(1)
    assert pathlib.Path(os.environ['RECORD'] + '.rustup').exists()
''')
        self.install("readelf", r'''#!/usr/bin/env python3
import os, sys
arm = 'aarch64' in sys.argv[-1]
option = sys.argv[1]
if os.environ.get('READELF_FAILURE'):
    sys.exit(5)
if option == '-lW':
    print('  LOAD 0x0 0x0 0x0 0x100 0x100 R E 0x1000')
    loader = '/lib/ld-linux-aarch64.so.1' if arm else '/lib64/ld-linux-x86-64.so.2'
    print('[Requesting program interpreter: ' + os.environ.get('LOADER', loader) + ']')
    if os.environ.get('NATIVE_TLS'):
        print('  TLS 0x0 0x0 0x0 0x10 0x10 R 0x8')
elif option == '-VW':
    version = os.environ.get('GLIBC_VERSION', '2.39' if arm else '2.17')
    if version:
        print('0x0010: Name: GLIBC_' + version + ' Flags: none Version: 2')
elif option == '-dW':
    print('0x00000001 (NEEDED) Shared library: [libc.so.6]')
    if os.environ.get('DYNAMIC_OPENSSL'):
        print('0x00000001 (NEEDED) Shared library: [libssl.so.3]')
elif option == '-sW':
    if not os.environ.get('NO_EMUTLS'):
        print('__emutls_get_address@GCC_4.3.0 __emutls_v._ZNfixture')
else:
    sys.exit(4)
''')

    def install(self, name, text):
        path = self.bin / name
        path.write_text(text)
        path.chmod(0o755)

    def command(self, *args):
        return subprocess.run(args, cwd=self.root, env=self.env, check=True,
                              text=True, capture_output=True).stdout.strip()

    def run_script(self, target=X86, **updates):
        env = {**self.env, **updates}
        return subprocess.run(["bash", str(self.root / "scripts" / SCRIPT.name), target],
                              cwd=self.root, env=env, text=True, capture_output=True)

    def records(self):
        path = Path(self.env["RECORD"])
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_portable_x86_command_and_release_metadata(self):
        result = self.run_script(JUSTRUST_TOOLCHAIN="system", CARGO_PROFILE_RELEASE_OPT_LEVEL="1",
                                 JCODE_BUILD_GIT_HASH="stale", JCODE_BUILD_GIT_DIRTY="true")
        self.assertEqual(result.returncode, 0, result.stderr)
        backend, build = self.records()
        self.assertEqual(backend["args"], ["remote", "use", "hosted"])
        self.assertEqual(build["args"], ["zigbuild", "--remote", "--locked", "--release", "--target",
                                       X86 + ".2.17", "-p", "jcode", "--bin", "jcode", "--features",
                                       "linux-compat-vendored-openssl"])
        expected = {"JUSTRUST_REMOTE_REQUIRED": "1", "JUSTRUST_TOOLCHAIN_REQUIRED": "1",
                    "JUSTRUST_TOOLCHAIN": "1.99.0", "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
                    "CARGO_INCREMENTAL": "0", "CARGO_BUILD_RUSTC_WRAPPER": "",
                    "JCODE_RELEASE_BUILD": "1", "JCODE_BUILD_SEMVER": "v1.0.0-preview.42",
                    "JCODE_BUILD_GIT_DIRTY": "false"}
        for key, value in expected.items():
            self.assertEqual(build["env"][key], value, key)
        self.assertEqual(build["env"]["JCODE_BUILD_GIT_HASH"], self.command("git", "rev-parse", "--short", "HEAD"))
        self.assertIn("fixture source", build["env"]["JCODE_BUILD_CHANGELOG_RAW"])
        self.assertTrue(build["env"]["JCODE_BUILD_GIT_DATE"])
        self.assertIn("GLIBC max 2.17", result.stdout)
        self.assertNotIn("--no-default-features", build["args"])
        self.assertNotIn("RUSTUP_TOOLCHAIN", build["env"])
        self.assertFalse(Path(self.env["RECORD"] + ".rustup").exists())

    def test_arm_rebuilds_std_with_target_only_tls(self):
        result = self.run_script(ARM, RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS="",
                                 CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-Cdebuginfo=1")
        self.assertEqual(result.returncode, 0, result.stderr)
        build = self.records()[-1]
        self.assertEqual(build["args"][0], "build")
        self.assertIn("-Zbuild-std=std,panic_abort", build["args"])
        self.assertEqual(build["env"]["JUSTRUST_TOOLCHAIN"], "nightly-2026-10-08")
        self.assertEqual(build["env"]["RUSTUP_TOOLCHAIN"], "nightly-2026-10-08")
        self.assertEqual(build["env"]["CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"],
                         "-Cdebuginfo=1 -Ztls-model=emulated")
        self.assertNotIn("RUSTFLAGS", build["env"])
        self.assertNotIn("CARGO_ENCODED_RUSTFLAGS", build["env"])
        self.assertIn("GLIBC max 2.39", result.stdout)

    def test_arm_overrides_stable_caller_without_changing_managed_contract(self):
        result = self.run_script(ARM, RUSTUP_TOOLCHAIN="1.99.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(Path(self.env["RECORD"] + ".rustup").read_text()),
                         ["toolchain", "install", "nightly-2026-10-08", "--profile", "minimal",
                          "--no-self-update"])
        build = self.records()[-1]
        for key in ["JUSTRUST_REMOTE_REQUIRED", "JUSTRUST_TOOLCHAIN_REQUIRED"]:
            self.assertEqual(build["env"][key], "1")
        self.assertNotIn("RUSTC_BOOTSTRAP", build["env"])
        self.assertNotIn("--local", build["args"])
        self.assertNotIn("--no-default-features", build["args"])

    def test_stable_metadata_fixture_rejects_native_target_tls(self):
        # A managed nightly pin alone must not make the caller fixture pass.
        env = {**self.env, "JUSTRUST_TOOLCHAIN": "nightly-2026-10-08",
               "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS": "-Ztls-model=emulated"}
        result = subprocess.run([str(self.bin / "cargo"), "metadata", "--format-version", "1",
                                 "--offline"], cwd=self.root, env=env, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("only accepted on the nightly compiler", result.stderr)

    def test_arm_nightly_install_failure_stops_before_hosted_dispatch(self):
        result = self.run_script(ARM, RUSTUP_FAILURE="19")
        self.assertEqual(result.returncode, 19, result.stderr)
        self.assertFalse(self.records())

    def test_dirty_source_is_reported_honestly(self):
        (self.root / "untracked.txt").write_text("dirty")
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.records()[-1]["env"]["JCODE_BUILD_GIT_DIRTY"], "true")

    def test_invalid_inputs_fail_before_hosted_dispatch(self):
        for key, value in [("JCODE_BUILD_SEMVER", ""), ("JCODE_BUILD_SEMVER", "v1.0.0-preview.0"),
                           ("JCODE_BUILD_SEMVER", "v1.0.0-preview.01"),
                           ("JCODE_BUILD_SEMVER", "v1.0.0-preview.18446744073709551616"),
                           ("JCODE_BUILD_SEMVER", "v1.1.0-preview.2"),
                           ("RUSTFLAGS", "-Ztls-model=emulated"),
                           ("CARGO_ENCODED_RUSTFLAGS", "-Clinker=mold"), ("RUSTC_BOOTSTRAP", "1"),
                           ("CARGO_TARGET_DIR", "other"), ("CARGO_BUILD_TARGET_DIR", "other"),
                           ("JUSTRUST_DISABLE", "1")]:
            with self.subTest(key=key):
                result = self.run_script(**{key: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.records())
        result = self.run_script("aarch64-unknown-linux-musl")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.records())

    def test_hosted_failure_never_reuses_stale_artifact_or_falls_back(self):
        self.assertEqual(self.run_script().returncode, 0)
        result = self.run_script(BUILD_FAILURE="1")
        self.assertEqual(result.returncode, 23)
        self.assertNotIn("Validated hosted", result.stdout)
        self.assertNotIn("--local", self.records()[-1]["args"])

    def test_backend_failure_prevents_build(self):
        result = self.run_script(BACKEND_FAILURE="17")
        self.assertEqual(result.returncode, 17)
        self.assertEqual(len(self.records()), 1)

    def test_missing_download_fails(self):
        result = self.run_script(NO_ARTIFACT="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("did not return an executable", result.stderr)

    def test_x86_rejects_bad_abi(self):
        for updates in [{"INVALID_ELF": "1"}, {"MACHINE": "183"}, {"GLIBC_VERSION": "2.18"},
                        {"GLIBC_VERSION": ""}, {"GLIBC_VERSION": "PRIVATE"},
                        {"LOADER": "/lib/ld-musl-x86_64.so.1"}, {"DYNAMIC_OPENSSL": "1"},
                        {"READELF_FAILURE": "1"}]:
            with self.subTest(updates=updates):
                result = self.run_script(**updates)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("Validated hosted", result.stdout)

    def test_arm_rejects_native_tls_missing_emutls_and_newer_glibc(self):
        for updates in [{"NATIVE_TLS": "1"}, {"NO_EMUTLS": "1"}, {"GLIBC_VERSION": "2.40"},
                        {"MACHINE": "62"}, {"DYNAMIC_OPENSSL": "1"}]:
            with self.subTest(updates=updates):
                result = self.run_script(ARM, **updates)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("Validated hosted", result.stdout)


if __name__ == "__main__":
    unittest.main()
