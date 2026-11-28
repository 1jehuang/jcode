"""Exercise the hosted-client binary installer without a network.

The installer downloads a verified release tarball, extracts the client
binary, and only then publishes it to GITHUB_PATH. The fakes below serve
archive + checksum files from a local directory so every failure stage
(failed download, bad checksum, broken binary) can be asserted without
touching the network.
"""
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Matches the installer's expectation: justrust-<target>/justrust inside the
# archive. The "binary" is a shell script so it runs on any dev platform.
CLIENT_SH = """#!/bin/sh
[ "$1" = --version ] && exit 0
exit "${CAPABILITY_STATUS:-0}"
"""


class HostedClientBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.releases = self.root / "releases"
        self.releases.mkdir()
        # The installer picks the musl target from uname -m, so the fake
        # release must match the host arch or the curl stub 404s.
        target = (
            "aarch64-unknown-linux-musl"
            if os.uname().machine in ("arm64", "aarch64")
            else "x86_64-unknown-linux-musl"
        )
        self.write_archive(target)
        self.write_curl_stub()
        self.write_sha256sum_stub()
        (self.root / "runner").mkdir()
        self.env = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "RUNNER_TEMP": str(self.root / "runner"),
            "GITHUB_PATH": str(self.root / "path"),
        }

    def write_tool(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def write_archive(self, target: str = "x86_64-unknown-linux-musl"):
        """Create the release archive + checksum pair in self.releases."""
        member = f"justrust-{target}"
        staging = self.root / "stage" / member
        staging.mkdir(parents=True, exist_ok=True)
        client = staging / "justrust"
        client.write_text(CLIENT_SH)
        client.chmod(0o755)
        archive = self.releases / f"{member}.tar.gz"
        subprocess.run(
            ["tar", "-czf", str(archive), "-C", str(self.root / "stage"), member],
            check=True,
        )
        digest = subprocess.run(
            ["shasum", "-a", "256", str(archive)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.split()[0]
        (self.releases / f"{archive.name}.sha256").write_text(
            f"{digest}  {archive.name}\n"
        )

    def write_curl_stub(self):
        # curl -fsSL ... -o <path> <url>: map the URL basename onto the local
        # releases dir and copy the file there. exit 22 mimics curl's HTTP 404.
        self.write_tool(
            "curl",
            "#!/bin/sh\n"
            'path=""; url=""; prev=""\n'
            'for arg in "$@"; do\n'
            '  if [ "$prev" = "-o" ]; then path="$arg"; fi\n'
            '  case "$arg" in http://*|https://*) url="$arg";; esac\n'
            '  prev="$arg"\n'
            "done\n"
            'if [ -z "$path" ] || [ -z "$url" ]; then\n'
            '  echo "curl stub: missing -o <path> or url" >&2; exit 3\n'
            "fi\n"
            'src="$(dirname "$0")/../releases/$(basename "$url")"\n'
            'if [ ! -f "$src" ]; then exit 22; fi\n'
            'cp "$src" "$path"\n',
        )

    def write_sha256sum_stub(self):
        # sha256sum --check <file>: each line is "<hash>  <name>" relative to
        # the checksum file's directory. Delegate the digest to the real
        # shasum so the stub only owns the verification loop.
        self.write_tool(
            "sha256sum",
            "#!/bin/sh\n"
            'check_file=""; prev=""\n'
            'for arg in "$@"; do\n'
            '  if [ "$prev" = "-c" ] || [ "$prev" = "--check" ]; then check_file="$arg"; fi\n'
            '  prev="$arg"\n'
            "done\n"
            '[ -n "$check_file" ] || { echo "sha256sum stub: no --check file" >&2; exit 2; }\n'
            'cd "$(dirname "$check_file")" || exit 2\n'
            'status=0\n'
            'while read -r want name; do\n'
            '  [ -n "$want" ] || continue\n'
            '  if [ ! -f "$name" ]; then status=1; continue; fi\n'
            '  got=$(/usr/bin/shasum -a 256 "$name" | cut -d" " -f1)\n'
            '  if [ "$got" != "$want" ]; then status=1; fi\n'
            "done < \"$(basename \"$check_file\")\"\n"
            'exit "$status"\n',
        )

    def run_installer(self, **extra):
        env = {key: value for key, value in self.env.items() if key != "CAPABILITY_STATUS"}
        env.update(extra)
        return subprocess.run(
            ["bash", str(ROOT / "scripts/install_justrust_hosted_ci.sh")],
            env=env,
            text=True,
            capture_output=True,
        )

    def test_installs_verified_binary_and_publishes_path(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        published = (self.root / "path").read_text().strip()
        self.assertTrue(published.endswith("/install/bin"), published)
        installed = Path(published) / "justrust"
        self.assertTrue(installed.exists())

    def test_bad_checksum_aborts_without_activation(self):
        name = "justrust-" + (
            "aarch64-unknown-linux-musl"
            if os.uname().machine in ("arm64", "aarch64")
            else "x86_64-unknown-linux-musl"
        )
        (self.releases / f"{name}.tar.gz.sha256").write_text(
            "0" * 64 + f"  {name}.tar.gz\n"
        )
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "path").exists())

    def test_missing_archive_aborts_without_activation(self):
        shutil.rmtree(self.releases)
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "path").exists())

    def test_broken_binary_aborts_without_activation(self):
        # The installer smoke-tests the downloaded binary (--version) before
        # publishing it to GITHUB_PATH; a binary that fails the smoke test
        # must abort the install.
        member = "justrust-" + (
            "aarch64-unknown-linux-musl"
            if os.uname().machine in ("arm64", "aarch64")
            else "x86_64-unknown-linux-musl"
        )
        staging = self.root / "stage" / member
        client = staging / "justrust"
        client.write_text("#!/bin/sh\nexit 7\n")
        client.chmod(0o755)
        archive = self.releases / f"{member}.tar.gz"
        subprocess.run(
            ["tar", "-czf", str(archive), "-C", str(self.root / "stage"), member],
            check=True,
        )
        digest = subprocess.run(
            ["shasum", "-a", "256", str(archive)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.split()[0]
        (self.releases / f"{archive.name}.sha256").write_text(
            f"{digest}  {archive.name}\n"
        )
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "path").exists())


if __name__ == "__main__":
    unittest.main()