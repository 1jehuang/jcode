#!/usr/bin/env python3
"""Exercise the real dev Cargo wrapper without compiling either workspace."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parent.parent
WRAPPER = REPO / "scripts/dev_cargo.sh"


class CargoWorkingDirectoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.tmp.cleanup)
        self.project = Path(self.tmp.name) / "other rust project"
        (self.project / "src").mkdir(parents=True)
        (self.project / "Cargo.toml").write_text(
            '[package]\nname = "cwd_probe"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        (self.project / "src/lib.rs").write_text("")
        self.env = dict(os.environ, JCODE_RUST_ACTION_LOG="0")
        self.env.pop("CARGO_MANIFEST_DIR", None)

    def metadata(self, cwd, *, child_shell=False):
        args = [str(WRAPPER), "metadata", "--no-deps", "--format-version", "1"]
        if child_shell:
            # Match BashTool's exported shim, including a subsequent cd and
            # inheritance by a child shell.
            args = ["bash", "-c", '''
                cargo() {
                    if [[ "${JCODE_IN_DEV_CARGO:-0}" == "1" ]]; then
                        command cargo "$@"
                    else
                        JCODE_IN_DEV_CARGO=1 "$JCODE_DEV_CARGO_SCRIPT" "$@"
                    fi
                }
                export -f cargo
                cd "$1"
                bash -c 'cargo metadata --no-deps --format-version 1'
            ''', "probe", str(cwd)]
            cwd = REPO
        result = subprocess.run(
            args, cwd=cwd,
            env=dict(self.env, JCODE_DEV_CARGO_SCRIPT=str(WRAPPER)),
            capture_output=True, text=True, timeout=60, check=True,
        )
        return json.loads(result.stdout)

    def test_foreign_project_keeps_its_workspace(self):
        metadata = self.metadata(self.project)
        self.assertEqual(Path(metadata["workspace_root"]), self.project)
        self.assertEqual(metadata["packages"][0]["name"], "cwd_probe")

    def test_exported_shim_after_cd_in_child_shell(self):
        self.assertEqual(Path(self.metadata(self.project, child_shell=True)["workspace_root"]), self.project)

    def test_jcode_root_and_member_keep_wrapper_policy(self):
        for cwd in [REPO, REPO / "crates/jcode-tui"]:
            with self.subTest(cwd=cwd):
                self.assertEqual(Path(self.metadata(cwd)["workspace_root"]), REPO)

    def test_symlink_to_foreign_project_keeps_its_workspace(self):
        link = Path(self.tmp.name) / "linked project"
        link.symlink_to(self.project, target_is_directory=True)
        self.assertEqual(Path(self.metadata(link)["workspace_root"]), self.project)

    def test_foreign_cargo_errors_are_preserved(self):
        result = subprocess.run(
            [str(WRAPPER), "metadata", "--manifest-path", "missing.toml"],
            cwd=self.project, env=self.env, capture_output=True, text=True, timeout=60,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing.toml", result.stderr)


if __name__ == "__main__":
    unittest.main()
