"""Execute portable packaging with a fake container boundary, not a fake compiler."""
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class HostedPackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="jcode hosted package ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        shutil.copy2(ROOT / "scripts/package_preview_linux_hosted.sh", self.root / "scripts")
        self.binary = self.root / "target/x86_64-unknown-linux-gnu/release/jcode"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text('#!/bin/sh\nprintf "arg=<%s>\\n" "$@"\nprintf "libs=<%s>\\n" "$LD_LIBRARY_PATH"\n')
        self.binary.chmod(0o755)
        self.bin = self.root / "tools"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$DOCKER_LOG"\nexit "${DOCKER_STATUS:-0}"\n')
        docker.chmod(0o755)
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}",
                    "DOCKER_LOG": str(self.root / "docker.log")}

    def run_package(self, *args, **extra):
        return subprocess.run(["bash", str(self.root / "scripts/package_preview_linux_hosted.sh"), *args],
                              env={**self.env, **extra}, text=True, capture_output=True)

    def test_exact_archive_layout_and_wrapper_arguments(self):
        result = self.run_package()
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = self.root / "dist/jcode-linux-x86_64.tar.gz"
        with tarfile.open(archive) as package:
            self.assertEqual(package.getnames(), ["jcode-linux-x86_64", "jcode-linux-x86_64.bin"])
            self.assertEqual(package.extractfile("jcode-linux-x86_64.bin").read(), self.binary.read_bytes())
        wrapper = self.root / "dist/jcode-linux-x86_64"
        alias = self.root / "alias"
        alias.symlink_to(wrapper)
        result = subprocess.run([str(alias), "--version", "space argument"],
                                env={**self.env, "LD_LIBRARY_PATH": "/existing"},
                                text=True, capture_output=True, check=True)
        self.assertIn("arg=<space argument>", result.stdout)
        self.assertIn(f"libs=<{self.root}/dist:/existing>", result.stdout)
        self.assertFalse(list((self.root / "dist").glob(".hosted-linux-package.*")))

    def test_container_is_pinned_and_sandboxed_with_all_smokes(self):
        self.assertEqual(self.run_package().returncode, 0)
        args = (self.root / "docker.log").read_text().splitlines()
        for item in ("--read-only", "--cap-drop", "ALL", "--network", "none",
                     "no-new-privileges", "65534:65534"):
            self.assertIn(item, args)
        self.assertTrue(any("manylinux2014_x86_64@sha256:" in arg for arg in args))
        self.assertTrue(any("dst=/input/jcode,readonly" in arg for arg in args))
        script = (self.root / "docker.log").read_text()
        for command in ("receive", "credentials", "router-credentials", "cancel-return"):
            self.assertIn(command, script)
        self.assertNotIn("JCODE_API_KEY", script)

    def test_failed_baseline_does_not_package(self):
        result = self.run_package(DOCKER_STATUS="42")
        self.assertEqual(result.returncode, 42)
        self.assertFalse((self.root / "dist").exists())

    def test_missing_executable_and_extra_arguments_fail_closed(self):
        self.binary.unlink()
        self.assertNotEqual(self.run_package().returncode, 0)
        self.assertNotEqual(self.run_package("unexpected").returncode, 0)
        self.assertFalse((self.root / "docker.log").exists())


if __name__ == "__main__":
    unittest.main()
