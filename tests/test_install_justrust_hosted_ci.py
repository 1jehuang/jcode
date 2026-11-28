"""Exercise the verified hosted-client download without network access."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class HostedClientBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for arch in ["x86_64", "aarch64"]:
            name = f"justrust-{arch}-unknown-linux-musl"
            archive = self.root / f"{name}.tar.gz"
            binary = b'#!/bin/sh\nprintf "%s\\n" "$*" >> "$VERSION_LOG"\nexit "${VERSION_STATUS:-0}"\n'
            with tarfile.open(archive, "w:gz") as package:
                member = tarfile.TarInfo(f"{name}/justrust")
                member.size = len(binary)
                package.addfile(member, io.BytesIO(binary))
            archive.with_suffix(".gz.sha256").write_text(
                f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n"
            )
        self.write_tool("uname", '#!/bin/sh\nprintf "%s\\n" "${MACHINE:-x86_64}"\n')
        self.write_tool("curl", '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
root = pathlib.Path(os.environ["RUNNER_TEMP"])
with (root / "calls").open("a") as log:
    log.write(" ".join(args) + "\\n")
if os.environ.get("DOWNLOAD_STATUS"):
    sys.exit(int(os.environ["DOWNLOAD_STATUS"]))
destination = pathlib.Path(args[args.index("-o") + 1])
shutil.copyfile(root / args[-1].rsplit("/", 1)[-1], destination)
if os.environ.get("CORRUPT") and destination.name.endswith(".tar.gz"):
    with destination.open("ab") as archive:
        archive.write(b"corrupt")
''')
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}",
                    "RUNNER_TEMP": str(self.root), "GITHUB_PATH": str(self.root / "path"),
                    "VERSION_LOG": str(self.root / "executed")}

    def write_tool(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def run_installer(self, **extra):
        return subprocess.run(["bash", str(ROOT / "scripts/install_justrust_hosted_ci.sh")],
                              env={**self.env, **extra}, text=True, capture_output=True)

    def test_verified_binary_for_each_supported_architecture(self):
        for machine, arch in [("x86_64", "x86_64"), ("arm64", "aarch64")]:
            with self.subTest(machine=machine):
                result = self.run_installer(MACHINE=machine)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"justrust-{arch}-unknown-linux-musl.tar.gz", (self.root / "calls").read_text())
                installed = Path((self.root / "path").read_text().splitlines()[-1]) / "justrust"
                self.assertTrue(installed.is_file())
                self.assertIn("--version", (self.root / "executed").read_text())

    def test_checksum_failure_never_executes_or_activates(self):
        self.assertNotEqual(self.run_installer(CORRUPT="1").returncode, 0)
        self.assertFalse((self.root / "executed").exists())
        self.assertFalse((self.root / "path").exists())

    def test_download_failure_never_executes_or_activates(self):
        self.assertEqual(self.run_installer(DOWNLOAD_STATUS="22").returncode, 22)
        self.assertFalse((self.root / "executed").exists())
        self.assertFalse((self.root / "path").exists())

    def test_binary_failure_never_activates(self):
        self.assertEqual(self.run_installer(VERSION_STATUS="42").returncode, 42)
        self.assertFalse((self.root / "path").exists())

    def test_unsupported_architecture_never_downloads(self):
        self.assertNotEqual(self.run_installer(MACHINE="unsupported").returncode, 0)
        self.assertFalse((self.root / "calls").exists())
        self.assertFalse((self.root / "path").exists())


if __name__ == "__main__":
    unittest.main()
