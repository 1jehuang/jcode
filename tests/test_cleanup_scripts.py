#!/usr/bin/env python3
"""Full cleanup-script regressions. Uses only disposable temporary caches."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

SCRIPT = Path(os.environ.get("JCODE_TEST_CLEANUP_SCRIPT", str(
    Path(__file__).resolve().parents[1] / "scripts/clean_target.sh")))


class CleanupScriptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="jcode-cleanup-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.target = self.root / "cache space" / "target"
        self.target.mkdir(parents=True)
        self.sibling = self.root / "cache"
        self.sibling.mkdir()
        (self.sibling / "sentinel").write_text("preserve")
        stamp = time.time() - 12 * 86400
        os.utime(self.sibling / "sentinel", (stamp, stamp))
        os.utime(self.sibling, (stamp, stamp))

    def stale(self, path, age=10):
        path.mkdir(parents=True)
        timestamp = time.time() - age * 86400
        os.utime(path, (timestamp, timestamp))
        return path

    def run_script(self, *args):
        env = dict(os.environ, CARGO_TARGET_DIR=str(self.target))
        return subprocess.run(["bash", str(SCRIPT), *args], env=env,
                              text=True, capture_output=True, timeout=30)

    def sessions(self, profile="debug", crate="crate"):
        parent = self.target / profile / "incremental"
        old = self.stale(parent / (crate + "-old"), 12)
        newest = self.stale(parent / (crate + "-new"), 10)
        return old, newest

    def test_dry_run_preserves_full_space_path(self):
        old, _ = self.sessions()
        result = self.run_script("--sweep-only", "--sweep", "2")
        self.assertIn(f"would remove (stale incremental session (>2d)): {old}  [", result.stderr)

    def test_apply_preserves_sibling_and_newest(self):
        old, newest = self.sessions()
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists(), newest.exists(),
                          (self.sibling / "sentinel").read_text()), (0, False, True, "preserve"))

    def test_release_lto_is_swept(self):
        old, newest = self.sessions("release-lto")
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists(), newest.exists()), (0, False, True))

    def test_incremental_names_with_tabs_and_newlines(self):
        old, newest = self.sessions(crate="crate\twith\nline")
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists(), newest.exists()), (0, False, True))

    def test_profile_symlink_cannot_escape_target(self):
        self.target = self.root / "target"
        self.target.mkdir()
        outside = self.root / "outside"
        old = self.stale(outside / "incremental" / "crate-old", 12)
        self.stale(outside / "incremental" / "crate-new", 10)
        (self.target / "debug").symlink_to(outside, target_is_directory=True)
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists()), (0, True))

    def test_incremental_parent_symlink_cannot_escape_target(self):
        outside = self.root / "outside"
        old = self.stale(outside / "crate-old", 12)
        self.stale(outside / "crate-new", 10)
        profile = self.target / "debug"
        profile.mkdir()
        (profile / "incremental").symlink_to(outside, target_is_directory=True)
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists()), (0, True))

    def test_dependency_parent_symlink_cannot_escape_target(self):
        self.target = self.root / "target"
        self.target.mkdir()
        outside = self.root / "outside"
        outside.mkdir()
        paths = []
        for index, age in enumerate((12, 10)):
            path = outside / ("libcrate-" + str(index) * 16 + ".rlib")
            path.write_text("preserve")
            stamp = time.time() - age * 86400
            os.utime(path, (stamp, stamp))
            paths.append(path)
        profile = self.target / "debug"
        profile.mkdir()
        (profile / "deps").symlink_to(outside, target_is_directory=True)
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, paths[0].exists()), (0, True))

    def test_filesystem_root_is_rejected_before_any_sweep(self):
        self.target = Path("/")
        result = self.run_script("--sweep-only", "--sweep", "2")
        self.assertEqual(result.returncode, 2, result.stderr)

    def test_target_with_trailing_newline_is_preserved(self):
        self.target = self.root / "target\n"
        self.target.mkdir()
        old, newest = self.sessions()
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists(), newest.exists()), (0, False, True))

    def test_empty_profiles_are_safe(self):
        (self.target / "debug" / "deps").mkdir(parents=True)
        (self.target / "debug" / "incremental").mkdir()
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_invalid_sweep_argument_fails_before_removal(self):
        old, _ = self.sessions()
        result = self.run_script("--apply", "--sweep", "invalid")
        self.assertEqual((result.returncode, old.exists()), (2, True))

    def test_cross_compile_symlink_is_not_followed(self):
        outside = self.stale(self.root / "outside")
        (outside / "sentinel").write_text("preserve")
        (self.target / "aarch64-apple-darwin").symlink_to(outside, target_is_directory=True)
        result = self.run_script("--apply")
        self.assertEqual((result.returncode, (self.target / "aarch64-apple-darwin").is_symlink()), (0, True))

    def test_deps_preserve_spaces_tabs_newlines(self):
        deps = self.target / "debug" / "deps"
        deps.mkdir(parents=True)
        paths = []
        for index, age in enumerate((12, 10)):
            path = deps / ("libcrate \t\n-" + str(index) * 16 + ".rlib")
            path.write_bytes(b"artifact")
            stamp = time.time() - age * 86400
            os.utime(path, (stamp, stamp))
            paths.append(path)
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, paths[0].exists(), paths[1].exists()), (0, False, True))

    def test_recent_incremental_files_are_preserved(self):
        old, _ = self.sessions()
        (old / "active").write_text("recent")
        result = self.run_script("--apply", "--sweep-only", "--sweep", "2")
        self.assertEqual((result.returncode, old.exists()), (0, True))


if __name__ == "__main__":
    unittest.main()
