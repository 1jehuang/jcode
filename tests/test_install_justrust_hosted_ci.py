"""Exercise immutable hosted-client bootstrap without a network or compiler."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REVISION = "5c8c6e653d1874923cf94b93dd96aa3d312343f0"


class HostedClientBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.write_tool("git", '''#!/bin/sh
printf '%s\\n' "$*" >> "$BOOTSTRAP_LOG"
case "$*" in
  *"rev-parse HEAD"*) printf '%s\\n' "${RETURNED_REVISION}" ;;
esac
''')
        self.write_tool("cargo", '''#!/bin/sh
printf '%s\\n' "$*" >> "$BOOTSTRAP_LOG"
[ "${CARGO_STATUS:-0}" = 0 ] || exit "$CARGO_STATUS"
while [ "$#" -gt 0 ]; do
  if [ "$1" = --root ]; then shift; root=$1; fi
  shift
done
mkdir -p "$root/bin"
printf '#!/bin/sh\\n[ "$1" = --version ] && exit 0\\nexit "${CAPABILITY_STATUS:-0}"\\n' > "$root/bin/justrust"
chmod +x "$root/bin/justrust"
''')
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}",
                    "RUNNER_TEMP": str(self.root), "GITHUB_PATH": str(self.root / "path"),
                    "BOOTSTRAP_LOG": str(self.root / "calls"), "RETURNED_REVISION": REVISION}

    def write_tool(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def run_installer(self, **extra):
        return subprocess.run(["bash", str(ROOT / "scripts/install_justrust_hosted_ci.sh")],
                              env={**self.env, **extra}, text=True, capture_output=True)

    def test_exact_source_pin_locked_compiler_and_embedded_helper(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = (self.root / "calls").read_text()
        self.assertIn(f"--depth=1 --filter=blob:none origin {REVISION}", calls)
        self.assertIn("/scripts/sign-release.py", calls)
        self.assertIn("+1.99.0 install --path", calls)
        self.assertIn("--locked --root", calls)
        self.assertNotIn("toolchain/", calls)
        self.assertTrue((self.root / "path").read_text().strip().endswith("/install/bin"))

    def test_wrong_revision_never_compiles_or_activates(self):
        result = self.run_installer(RETURNED_REVISION="0" * 40)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("install --path", (self.root / "calls").read_text())
        self.assertFalse((self.root / "path").exists())

    def test_compiler_failure_never_activates(self):
        self.assertEqual(self.run_installer(CARGO_STATUS="42").returncode, 42)
        self.assertFalse((self.root / "path").exists())

    def test_missing_capability_never_activates(self):
        self.assertNotEqual(self.run_installer(CAPABILITY_STATUS="2").returncode, 0)
        self.assertFalse((self.root / "path").exists())


if __name__ == "__main__":
    unittest.main()
