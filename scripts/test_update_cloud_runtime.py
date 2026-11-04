#!/usr/bin/env python3
"""Regression tests for the cloud runtime stager using a real local shell."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import update_cloud_runtime as runtime
from update_cloud_runtime import install_script


class RuntimeUpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.root = self.home / ".local/share/jcode/cloud-releases"

    def archive(self, succeeds=True, reject_backend=False):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as tar:
            data = b"#!/bin/sh\nexit 0\n" if succeeds else b"#!/bin/sh\nexit 42\n"
            if reject_backend:
                data = b'#!/bin/sh\n[ "$(basename "$0")" != backend ]\n'
            member = tarfile.TarInfo("jcode")
            member.mode = 0o755
            member.size = len(data)
            tar.addfile(member, io.BytesIO(data))
        return output.getvalue()

    def run_install(self, data, checksum=None):
        digest = checksum or hashlib.sha256(data).hexdigest()
        result = subprocess.run(["bash", "-c", install_script(digest)], input=data,
                                env={**os.environ, "HOME": str(self.home)}, capture_output=True)
        return result, self.root / ("runtime-" + digest)

    def test_stage_is_idempotent_and_does_not_switch_launcher(self):
        data = self.archive()
        result, release = self.run_install(data)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((release / "jcode").exists())
        self.assertIn(b"STAGED_RUNTIME=", result.stdout)
        result, _ = self.run_install(data)
        self.assertEqual(result.returncode, 0, result.stderr)
        script = install_script(hashlib.sha256(data).hexdigest())
        self.assertNotIn("sudo", script)
        self.assertNotIn("/opt/jcode-cloud", script)
        self.assertFalse(list(self.root.glob(".upload.*")))

    def test_corrupt_transfer_rejected_before_publication(self):
        result, release = self.run_install(self.archive(), "0" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(release.exists())
        self.assertFalse(list(self.root.glob(".upload.*")))

    def test_failed_candidate_never_published(self):
        result, release = self.run_install(self.archive(False))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(release.exists())

    def test_modified_existing_release_rejected(self):
        data = self.archive()
        _, release = self.run_install(data)
        (release / "jcode").write_text("changed")
        result, _ = self.run_install(data)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((release / "jcode").read_text(), "changed")

    def test_concurrent_lock_preserved(self):
        self.root.mkdir(parents=True)
        lock = self.root / ".runtime-update-lock"
        lock.mkdir()
        result, release = self.run_install(self.archive())
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(lock.exists())
        self.assertFalse(release.exists())

    def run_activation(self, data):
        digest = hashlib.sha256(data).hexdigest()
        backend = self.home / "backend"
        old = self.home / "old-runtime"
        old.write_text("#!/bin/sh\nexit 0\n")
        old.chmod(0o755)
        backend.symlink_to(old)
        fake_bin = self.home / "bin"
        fake_bin.mkdir()
        sudo = fake_bin / "sudo"
        sudo.write_text('#!/bin/sh\n[ "$1" = -n ] && shift\nexec "$@"\n')
        sudo.chmod(0o755)
        script = install_script(digest, True).replace("/opt/jcode-cloud/jcode", str(backend))
        result = subprocess.run(["bash", "-c", script], input=data,
                                env={**os.environ, "HOME": str(self.home),
                                     "PATH": str(fake_bin) + ":" + os.environ["PATH"]},
                                capture_output=True)
        return result, backend, old

    def test_explicit_activation_switches_only_launcher_and_retains_previous(self):
        result, backend, old = self.run_activation(self.archive())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(backend.resolve(), old)
        self.assertTrue(old.exists())
        self.assertIn(b"Daemon unchanged", result.stdout)
        backups = list(self.root.glob("previous-backend-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text().strip(), str(old))

    def test_activation_failure_rolls_back_launcher(self):
        result, backend, old = self.run_activation(self.archive(reject_backend=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(backend.resolve(), old)
        self.assertIn(b"restored previous backend", result.stderr)

    def invoke_cli(self, returncodes, activate=False):
        archive = self.home / "archive.tar.gz"
        archive.write_bytes(b"verified bundle bytes")
        uploaded = []
        codes = iter(returncodes)

        def ssh(command, stdin):
            uploaded.append(stdin.read())
            return subprocess.CompletedProcess(command, next(codes))

        args = ["update_cloud_runtime.py", "unused-binary"]
        if activate:
            args.append("--activate-launcher")
        with patch("sys.argv", args), patch.dict(os.environ, {"JCODE_SCRATCH_DIR": str(self.home)}), \
                patch.object(runtime, "bundle", return_value=(archive, "a" * 64)), \
                patch.object(runtime.subprocess, "run", side_effect=ssh), \
                patch.object(runtime.time, "sleep"):
            runtime.main()
        return uploaded

    def test_cli_retries_only_transport_failure_and_rewinds_bundle(self):
        uploaded = self.invoke_cli([255, 0])
        self.assertEqual(uploaded, [b"verified bundle bytes"] * 2)

    def test_cli_does_not_retry_remote_validation_failure(self):
        with self.assertRaises(subprocess.CalledProcessError) as error:
            self.invoke_cli([1])
        self.assertEqual(error.exception.returncode, 1)

    def test_cli_does_not_repeat_activation_after_transport_failure(self):
        with self.assertRaises(subprocess.CalledProcessError) as error:
            self.invoke_cli([255], activate=True)
        self.assertEqual(error.exception.returncode, 255)

    def test_invalid_digest_rejected(self):
        with self.assertRaises(ValueError):
            install_script("'; echo bad")


if __name__ == "__main__":
    unittest.main()
