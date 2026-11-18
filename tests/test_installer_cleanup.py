#!/usr/bin/env python3
"""Execute complete installers with disposable homes and boundary-only stubs.

No real Cargo, network, server, user cache, or installation is touched.
JCODE_TEST_INSTALLER_BASELINE=1 checks the original HEAD scripts instead.
The guarded publisher itself is validated by Rust integration tests, not this stub.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest

REPO = Path(__file__).resolve().parents[1]


class InstallerScriptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="jcode-install-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "checkout space"
        self.home = self.root / "home space"
        self.tools = self.root / "tools"
        self.home.mkdir()
        self.tools.mkdir()
        (self.repo / "scripts/lib").mkdir(parents=True)
        for name in ("install_release.sh", "install.sh", "clean_target.sh", "lib/configure_path.sh"):
            dest = self.repo / "scripts" / name
            if os.environ.get("JCODE_TEST_INSTALLER_BASELINE") == "1":
                dest.write_bytes(subprocess.check_output(["git", "show", "HEAD:scripts/" + name], cwd=REPO))
            else:
                shutil.copy2(REPO / "scripts" / name, dest)
            dest.chmod(0o755)
        self.log = self.root / "commands.jsonl"
        self.payload = self.root / "fake-jcode"
        self.executable(self.payload, '''
import json, os, pathlib, shutil, sys
args = sys.argv[1:]
with open(os.environ["FIXTURE_LOG"], "a") as f:
    f.write(json.dumps(args) + "\\n")
if args == ["--version"]:
    print("jcode 1.2.3 (fixture)")
    sys.exit(0)
if "publish-build" in args:
    if os.environ.get("FIXTURE_LEGACY") == "1":
        print("unrecognized subcommand publish-build", file=sys.stderr)
        sys.exit(2)
    def option(name):
        return args[args.index(name) + 1]
    source = pathlib.Path(option("--source"))
    builds = pathlib.Path(os.environ["JCODE_HOME"]) / "builds"
    version = option("--version")
    dest = builds / "versions" / version
    # Scripts must not create or copy into the version store before the helper.
    if dest.exists():
        print("unguarded version publication before helper", file=sys.stderr)
        sys.exit(90)
    with open(os.environ["FIXTURE_LOG"], "a") as f:
        f.write(json.dumps(["prepared-files"] + sorted(p.name for p in source.iterdir())) + "\\n")
    dest.mkdir(parents=True)
    for path in source.iterdir():
        shutil.copy2(path, dest / path.name)
    channel = option("--channel")
    for name in (["stable", "current"] if channel == "current" else ["stable"]):
        directory = builds / name
        directory.mkdir()
        (directory / "jcode").symlink_to(dest / "jcode")
        (builds / (name + "-version")).write_text(version + "\\n")
    launcher = pathlib.Path(option("--launcher"))
    launcher.parent.mkdir(parents=True, exist_ok=True)
    launcher.symlink_to(builds / channel / "jcode")
    sys.exit(0)
if "prune-builds" in args:
    print("No old builds to remove.")
    print("Failed to remove protected-fixture: permission denied")
    print("prune stderr diagnostic", file=sys.stderr)
    sys.exit(1)
sys.exit(0)
''')
        for profile in ("release", "release-lto"):
            dest = self.repo / "target" / profile / "jcode"
            dest.parent.mkdir(parents=True)
            shutil.copy2(self.payload, dest)
        self.executable(self.tools / "git", '''
import os, sys
args = sys.argv[1:]
if "--show-toplevel" in args: print(os.environ["FIXTURE_REPO"])
elif "--git-dir" in args: print(".git")
elif "--short" in args: print("fixture")
elif "log" in args: print("2026-10-12 00:00:00 +0000")
''')
        self.executable(self.tools / "cargo", '''
import json, os, sys
with open(os.environ["FIXTURE_LOG"], "a") as f:
    f.write(json.dumps(["cargo"] + sys.argv[1:]) + "\\n")
''')
        self.executable(self.tools / "uname", '''
import sys
print("x86_64" if "-m" in sys.argv else "Linux")
''')
        self.executable(self.tools / "curl", '''
import hashlib, os, pathlib, shutil, sys
args = sys.argv[1:]
url = next((a for a in args if a.startswith("https://")), "")
payload = pathlib.Path(os.environ.get("FIXTURE_ARCHIVE", os.environ["FIXTURE_PAYLOAD"]))
asset = "jcode-linux-x86_64.tar.gz" if "FIXTURE_ARCHIVE" in os.environ else "jcode-linux-x86_64"
if url.endswith("download-bases"):
    sys.exit(0)
if url.endswith("SHA256SUMS"):
    print(hashlib.sha256(payload.read_bytes()).hexdigest() + "  " + asset)
    sys.exit(0)
if url.endswith("/" + asset) and "-o" in args:
    shutil.copyfile(payload, args[args.index("-o") + 1])
    sys.exit(0)
sys.exit(22)
''')
        self.env = dict(os.environ, HOME=str(self.home),
                        PATH=str(self.tools) + ":/usr/bin:/bin",
                        XDG_CONFIG_HOME=str(self.home / ".config"),
                        JCODE_HOME=str(self.home / ".jcode"),
                        JCODE_INSTALL_DIR=str(self.home / "bin space"),
                        CARGO_TARGET_DIR=str(self.repo / "target"),
                        JCODE_VERSION="v1.2.3", JCODE_NO_TELEMETRY="1",
                        JCODE_KEEP_BUILDS="3", JCODE_SKIP_SERVER_RELOAD="1",
                        JCODE_SKIP_TARGET_SWEEP="1", FIXTURE_REPO=str(self.repo),
                        FIXTURE_PAYLOAD=str(self.payload), FIXTURE_LOG=str(self.log),
                        TMPDIR=str(self.root))
        # Keep inherited platform/shell configuration out of fixture behavior.
        for key in ("TERMUX_VERSION", "PREFIX", "JCODE_RELEASE_PROFILE", "JCODE_INSTALL_CONVERSION_ID"):
            self.env.pop(key, None)
        self.env["SHELL"] = "/bin/bash"

    def executable(self, path, source):
        path.write_text("#!" + sys.executable + "\n" + source)
        path.chmod(0o755)

    def run_installer(self, script, *args):
        return subprocess.run(["bash", str(self.repo / "scripts" / script), *args],
                              cwd=self.repo, env=self.env, capture_output=True,
                              text=True, timeout=30)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_release_installer_surfaces_prune_failure(self):
        result = self.run_installer("install_release.sh")
        self.assertEqual((result.returncode,
                          "Failed to remove protected-fixture" in result.stdout,
                          "prune stderr diagnostic" in result.stderr,
                          "Warning: build pruning failed" in result.stderr), (0, True, True, True), result)

    def test_download_installer_surfaces_prune_failure(self):
        result = self.run_installer("install.sh")
        self.assertEqual((result.returncode,
                          "Failed to remove protected-fixture" in result.stdout,
                          "prune stderr diagnostic" in result.stderr,
                          "Warning: build pruning failed" in result.stderr), (0, True, True, True), result)

    def test_release_prune_disables_background_updates(self):
        result = self.run_installer("install_release.sh", "--fast")
        self.assertEqual((result.returncode, ["--no-update", "prune-builds"] in self.commands()), (0, True))

    def test_download_prune_disables_background_updates(self):
        result = self.run_installer("install.sh")
        self.assertEqual((result.returncode, ["--no-update", "prune-builds"] in self.commands()), (0, True))

    def test_release_default_build_profile_is_lto(self):
        result = self.run_installer("install_release.sh")
        builds = [args for args in self.commands() if args[0] == "cargo"]
        self.assertEqual((result.returncode, builds[0][3]), (0, "release-lto"))

    def test_release_fast_build_profile_is_release(self):
        result = self.run_installer("install_release.sh", "--fast")
        builds = [args for args in self.commands() if args[0] == "cargo"]
        self.assertEqual((result.returncode, builds[0][3]), (0, "release"))

    def test_disabled_pruning_is_not_invoked(self):
        self.env["JCODE_KEEP_BUILDS"] = "0"
        result = self.run_installer("install_release.sh")
        self.assertEqual((result.returncode, any("prune-builds" in args for args in self.commands())), (0, False))

    def test_release_publication_uses_current_helper(self):
        result = self.run_installer("install_release.sh")
        calls = [args for args in self.commands() if "publish-build" in args]
        self.assertEqual((result.returncode, len(calls), calls[0][0],
                          calls[0][calls[0].index("--channel") + 1]), (0, 1, "--no-update", "current"))

    def test_download_publication_uses_stable_helper(self):
        result = self.run_installer("install.sh")
        calls = [args for args in self.commands() if "publish-build" in args]
        self.assertEqual((result.returncode, len(calls), calls[0][0],
                          calls[0][calls[0].index("--channel") + 1]), (0, 1, "--no-update", "stable"))

    def test_release_legacy_binary_fails_without_publication(self):
        self.env["FIXTURE_LEGACY"] = "1"
        result = self.run_installer("install_release.sh")
        self.assertEqual((result.returncode, "must support publish-build" in result.stderr,
                          (self.home / ".jcode/builds").exists()), (1, True, False))

    def test_download_legacy_binary_fails_without_publication(self):
        self.env["FIXTURE_LEGACY"] = "1"
        result = self.run_installer("install.sh")
        self.assertEqual((result.returncode, "requires a binary with publish-build" in result.stderr,
                          (self.home / ".jcode/builds").exists()), (1, True, False))

    def test_release_installer_automatically_sweeps_lto_incremental_cache(self):
        parent = self.repo / "target/release-lto/incremental"
        paths = []
        for name, age in (("crate-old", 12), ("crate-new", 10)):
            path = parent / name
            path.mkdir(parents=True)
            stamp = time.time() - age * 86400
            os.utime(path, (stamp, stamp))
            paths.append(path)
        self.env["JCODE_SKIP_TARGET_SWEEP"] = "0"
        result = self.run_installer("install_release.sh")
        self.assertEqual((result.returncode, paths[0].exists(), paths[1].exists()), (0, False, True), result)

    def test_archive_preparation_preserves_wrapper_payload_and_libraries(self):
        files = self.root / "archive-files"
        files.mkdir()
        wrapper = files / "jcode-linux-x86_64"
        wrapper.write_text('#!/usr/bin/env bash\nexec "$(dirname "$0")/jcode-linux-x86_64.bin" "$@"\n')
        wrapper.chmod(0o755)
        shutil.copy2(self.payload, files / "jcode-linux-x86_64.bin")
        (files / "libssl.so.3").write_text("ssl fixture")
        (files / "libcrypto.so.3").write_text("crypto fixture")
        archive = self.root / "fixture.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            for path in files.iterdir():
                tar.add(path, arcname=path.name)
        self.env["FIXTURE_ARCHIVE"] = str(archive)
        result = self.run_installer("install.sh")
        prepared = [args for args in self.commands() if args[0] == "prepared-files"]
        self.assertEqual((result.returncode, prepared),
                         (0, [["prepared-files", "jcode", "jcode-linux-x86_64.bin", "libcrypto.so.3", "libssl.so.3"]]), result)

    def test_termux_publication_requests_guarded_launcher_wrapper(self):
        self.env["TERMUX_VERSION"] = "fixture"
        result = self.run_installer("install.sh")
        calls = [args for args in self.commands() if "publish-build" in args]
        self.assertEqual((result.returncode, "--termux-launcher" in calls[0]), (0, True), result)


if __name__ == "__main__":
    unittest.main()
