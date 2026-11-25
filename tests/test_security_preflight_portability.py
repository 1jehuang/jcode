"""Run the tracked-file scan with macOS's system Bash and disposable files."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class SecurityPreflightTests(unittest.TestCase):
    def test_bash3_scans_complete_tracked_paths(self):
        source = Path(__file__).resolve().parents[1] / "scripts/security_preflight.sh"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            shutil.copyfile(source, root / "scripts/security_preflight.sh")
            names = ["source with spaces", "source\twith\nnewlines"]
            for name in names:
                (root / name).write_text("fixture")
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            subprocess.run(["git", "-C", str(root), "add", "--", *names], check=True)
            binaries = root / "bin"
            binaries.mkdir()
            for name, script in {
                "rg": '#!/usr/bin/env python3\nimport json, os, pathlib, sys\npathlib.Path(os.environ["SCAN_ARGS"]).write_text(json.dumps(sys.argv[1:]))\nsys.exit(1)\n',
                "cargo": "#!/bin/sh\nexit 0\n",
                "cargo-audit": "#!/bin/sh\nexit 0\n",
            }.items():
                path = binaries / name
                path.write_text(script)
                path.chmod(0o755)
            output = root / "scan-args.json"
            result = subprocess.run(
                ["/bin/bash", str(root / "scripts/security_preflight.sh"), "--strict"],
                env={**os.environ, "PATH": f"{binaries}:{os.environ['PATH']}", "SCAN_ARGS": str(output)},
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            arguments = json.loads(output.read_text())
            self.assertEqual(arguments[-len(names):], sorted(names))


if __name__ == "__main__":
    unittest.main()
