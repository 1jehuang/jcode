#!/usr/bin/env python3
"""Real CLI acceptance tests. Root supplies an already-built JCODE_TEST_BINARY.

Run: JCODE_TEST_BINARY=/absolute/path/to/jcode python3 tests/test_build_store_cli.py
No Cargo, daemon, network, or live installation is used. Each test owns its store.
The Rust installer barrier regression supplies deterministic gap coverage. These
checks exercise the compiled CLI, kernel-lock interoperability, and real overlap.
"""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

try:
    import fcntl
except ImportError:
    fcntl = None

TIMEOUT = 40
REPO = Path(__file__).resolve().parents[1]


class BuildStoreCliTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        value = os.environ.get("JCODE_TEST_BINARY")
        if not value:
            raise unittest.SkipTest("JCODE_TEST_BINARY must name a root-built executable")
        cls.binary = Path(value)
        if not cls.binary.is_absolute() or not cls.binary.is_file():
            raise RuntimeError("JCODE_TEST_BINARY must be an absolute executable file")
        if not os.access(cls.binary, os.X_OK):
            raise RuntimeError("JCODE_TEST_BINARY is not executable")
        cls.binary = cls.binary.resolve()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(
            prefix="jcode-cli-store-", dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "isolated home with spaces"
        self.builds = self.home / "builds"
        self.launcher = self.root / "custom launcher directory" / "jcode"
        self.payload = self.root / "prepared payload with spaces"
        self.payload.mkdir()
        self.home.mkdir()
        # Do not pass provider credentials, secret paths or the user's config.
        self.env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": str(self.home),
            "JCODE_HOME": str(self.home),
            "JCODE_INSTALL_DIR": str(self.launcher.parent),
            "JCODE_RUNTIME_DIR": str(self.root / "runtime"),
            "JCODE_NON_INTERACTIVE": "1",
            "JCODE_KEEP_BUILDS": "0",
            "JCODE_SKIP_SERVER_RELOAD": "1",
            "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_CACHE_HOME": str(self.root / "cache"),
            "XDG_DATA_HOME": str(self.root / "data"),
            "LANG": "C",
        }
        self.name = "jcode.exe" if os.name == "nt" else "jcode"
        (self.payload / self.name).write_bytes(b"isolated payload, never executed\n")

    def command(self, *args):
        return [str(self.binary), "--no-update", *map(str, args)]

    def run_cli(self, *args):
        return subprocess.run(self.command(*args), env=self.env,
                              cwd=self.root, capture_output=True, text=True,
                              timeout=TIMEOUT)

    def publish_args(self, version="release-new", channel="current", source=None):
        return ["publish-build", "--source", source or self.payload,
                "--version", version, "--channel", channel,
                "--launcher", self.launcher]

    def publish(self, version="release-new", channel="current", source=None):
        result = self.run_cli(*self.publish_args(version, channel, source))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return self.builds / "versions" / version / self.name

    def snapshot(self):
        result = {}
        for base in (self.builds, self.launcher.parent):
            if not base.exists():
                continue
            for path in sorted(base.rglob("*")):
                if path.name == ".store.lock":
                    continue
                if path.is_symlink():
                    result[str(path.relative_to(self.root))] = ("link", os.readlink(path))
                elif path.is_file():
                    result[str(path.relative_to(self.root))] = ("file", path.read_bytes())
        return result

    def start(self, *args):
        # Files avoid unbounded pipe buffering while processes wait on the lock.
        stdout = tempfile.TemporaryFile()
        stderr = tempfile.TemporaryFile()
        self.addCleanup(stdout.close)
        self.addCleanup(stderr.close)
        child = subprocess.Popen(self.command(*args), env=self.env, cwd=self.root,
                                 stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr)
        self.addCleanup(self.stop, child)
        return child, stdout, stderr

    @staticmethod
    def stop(child):
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)

    def finish(self, process):
        child, stdout, stderr = process
        try:
            status = child.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            self.stop(child)
            self.fail("real CLI exceeded bounded process deadline")
        stdout.seek(0)
        stderr.seek(0)
        self.assertEqual(status, 0, (stdout.read() + stderr.read()).decode(errors="replace"))

    def test_current_publication_updates_both_markers_and_launcher_with_spaces(self):
        installed = self.publish()
        self.assertEqual((self.builds / "stable-version").read_text().strip(), "release-new")
        self.assertEqual((self.builds / "current-version").read_text().strip(), "release-new")
        for path in (self.builds / "stable" / self.name,
                     self.builds / "current" / self.name, self.launcher):
            self.assertEqual(path.read_bytes(), installed.read_bytes())
            if os.name != "nt":
                self.assertEqual(path.resolve(), installed.resolve())

    def test_stable_publication_leaves_current_reference_unchanged(self):
        current = self.publish("current-old")
        (self.payload / self.name).write_bytes(b"new stable payload\n")
        stable = self.publish("stable-new", "stable")
        self.assertEqual((self.builds / "current-version").read_text().strip(), "current-old")
        self.assertEqual((self.builds / "current" / self.name).read_bytes(), current.read_bytes())
        self.assertEqual(self.launcher.read_bytes(), stable.read_bytes())

    def test_invalid_versions_and_channels_leave_existing_publication_unchanged(self):
        self.publish("existing")
        before = self.snapshot()
        for version, channel in (("../escape", "stable"), (".hidden", "current"),
                                 ("bad\\label", "stable"), ("valid", "legacy")):
            with self.subTest(version=version, channel=channel):
                result = self.run_cli(*self.publish_args(version, channel))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.snapshot(), before)

    def test_colliding_payload_fails_without_mutating_existing_publication(self):
        self.publish("existing")
        before = self.snapshot()
        (self.payload / self.name).write_bytes(b"conflicting replacement\n")
        result = self.run_cli(*self.publish_args("existing"))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(), before)

    def test_identical_republication_preserves_payload_identity(self):
        installed = self.publish("existing")
        before = installed.stat()
        self.publish("existing")
        after = installed.stat()
        self.assertEqual((after.st_ino, after.st_mtime_ns), (before.st_ino, before.st_mtime_ns))

    @unittest.skipUnless(fcntl is not None, "requires Unix kernel flock")
    def test_python_kernel_lock_blocks_publication_until_release(self):
        self.builds.mkdir()
        with (self.builds / ".store.lock").open("a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            process = self.start(*self.publish_args())
            # Complementary CLI check, not the deterministic gap proof: an early
            # command/parser failure is caught here, then success after release.
            with self.assertRaises(subprocess.TimeoutExpired):
                process[0].wait(timeout=0.5)
            self.assertFalse((self.builds / "versions" / "release-new").exists())
            fcntl.flock(lock, fcntl.LOCK_UN)
            self.finish(process)
        self.assertTrue(self.launcher.exists())

    @unittest.skipUnless(fcntl is not None, "requires Unix kernel flock")
    def test_concurrent_real_publication_and_keep_zero_pruning_preserve_active_build(self):
        self.publish("initial")
        old = self.builds / "versions" / "old-idle"
        old.mkdir()
        (old / self.name).write_bytes(b"unreferenced old binary\n")
        # Gate both processes behind the same kernel lock, then release together.
        # Run several overlaps. Rust's acknowledged barrier test covers the exact
        # rename-to-reference interleaving deterministically.
        for index in range(4):
            with (self.builds / ".store.lock").open("a+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                publisher = self.start(*self.publish_args(f"overlap-{index}"))
                pruner = self.start("prune-builds", "--keep", "0")
                fcntl.flock(lock, fcntl.LOCK_UN)
            self.finish(publisher)
            self.finish(pruner)
            installed = self.builds / "versions" / f"overlap-{index}" / self.name
            self.assertTrue(installed.is_file())
            self.assertEqual(self.launcher.resolve(), installed.resolve())
        self.assertFalse(old.exists(), "explicit keep zero must still remove old idle builds")
        result = self.run_cli("prune-builds", "--keep", "0")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(sorted(p.name for p in (self.builds / "versions").iterdir()
                                if not p.name.startswith(".")), ["overlap-3"])

    @unittest.skipIf(os.name == "nt", "requires Unix installer")
    def test_legacy_installer_binary_fails_closed_without_store_mutation(self):
        self.publish("existing")
        before = self.snapshot()
        repo = self.root / "legacy checkout with spaces"
        tools = self.root / "legacy tools"
        (repo / "target" / "release").mkdir(parents=True)
        tools.mkdir()
        scripts = {
            tools / "cargo": "#!/bin/sh\nexit 0\n",
            tools / "git": '#!/bin/sh\nif [ "$1" = rev-parse ]; then pwd; else exit 1; fi\n',
            tools / "date": "#!/bin/sh\nprintf 'legacy-version\\n'\n",
            repo / "target" / "release" / "jcode":
                "#!/bin/sh\necho 'unknown command publish-build' >&2\nexit 64\n",
        }
        for path, contents in scripts.items():
            path.write_text(contents)
            path.chmod(0o755)
        env = dict(self.env, PATH=str(tools) + os.pathsep + self.env["PATH"])
        result = subprocess.run(["bash", str(REPO / "scripts/install_release.sh"), "--fast"],
                                cwd=repo, env=env, capture_output=True, text=True, timeout=TIMEOUT)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Publication failed", result.stderr)
        self.assertEqual(self.snapshot(), before)


if __name__ == "__main__":
    unittest.main(verbosity=2)
