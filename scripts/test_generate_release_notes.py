#!/usr/bin/env python3

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("generate_release_notes.sh").resolve()


class GenerateReleaseNotesTests(unittest.TestCase):
    def _init_repo(self, root: Path) -> None:
        def git(*args: str) -> None:
            subprocess.run(
                ["git", *args],
                cwd=root,
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

        git("init", "-q")
        git("config", "user.name", "Fixture")
        git("config", "user.email", "fixture@example.invalid")
        git("remote", "add", "origin", "https://github.com/1jehuang/jcode.git")
        scripts_dir = root / "scripts"
        scripts_dir.mkdir()
        shutil.copy2(SCRIPT, scripts_dir / "generate_release_notes.sh")

        def commit(msg: str, tag: str | None = None) -> None:
            git("commit", "--allow-empty", "-qm", msg)
            if tag:
                git("tag", tag)

        commit("feat: initial release", "v1.0.1")
        commit("fix: preserve screenshot history", "v1.0.2")
        commit("feat: add Antigravity login", "v1.0.3")

        changelog_dir = root / "changelog"
        changelog_dir.mkdir()
        (changelog_dir / "index.json").write_text(
            json.dumps(
                {
                    "entries": [
                        {"version": "1.0.3", "date": "2026-11-22"},
                        {"version": "1.0.2", "date": "2026-11-19"},
                        {"version": "1.0.1", "date": "2026-11-18"},
                    ]
                }
            )
        )

    def test_skips_unpublished_draft_releases_from_github(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._init_repo(root)
            releases_fixture = json.dumps(
                [
                    {"tag_name": "v1.0.3", "draft": True, "prerelease": False},
                    {"tag_name": "v1.0.2", "draft": True, "prerelease": False},
                    {"tag_name": "v1.0.1", "draft": False, "prerelease": False},
                ]
            )
            env = {
                **os.environ,
                "JCODE_PUBLISHED_RELEASES_JSON": releases_fixture,
            }
            output = subprocess.check_output(
                ["bash", str(root / "scripts/generate_release_notes.sh"), "v1.0.3"],
                cwd=root,
                env=env,
                text=True,
            )
            # Must compare against published v1.0.1, NOT unpublished draft v1.0.2,
            # and must include the commit from v1.0.2 in fallback commit grouping.
            self.assertIn(
                "**Full changelog**: https://github.com/1jehuang/jcode/compare/v1.0.1...v1.0.3",
                output,
            )
            self.assertIn("preserve screenshot history", output)
            self.assertIn("add Antigravity login", output)

    def test_offline_index_skips_entries_marked_unpublished_or_draft(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._init_repo(root)
            (root / "changelog/index.json").write_text(
                json.dumps(
                    {
                        "entries": [
                            {"version": "1.0.3", "date": "2026-11-22"},
                            {"version": "1.0.2", "date": "2026-11-19", "published": False},
                            {"version": "1.0.1", "date": "2026-11-18"},
                        ]
                    }
                )
            )
            env = {**os.environ, "JCODE_SKIP_GITHUB_API": "1"}
            output = subprocess.check_output(
                ["bash", str(root / "scripts/generate_release_notes.sh"), "v1.0.3"],
                cwd=root,
                env=env,
                text=True,
            )
            self.assertIn(
                "**Full changelog**: https://github.com/1jehuang/jcode/compare/v1.0.1...v1.0.3",
                output,
            )

    def test_explicit_previous_tag_argument_wins(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._init_repo(root)
            env = {**os.environ, "JCODE_SKIP_GITHUB_API": "1"}
            output = subprocess.check_output(
                ["bash", str(root / "scripts/generate_release_notes.sh"), "v1.0.3", "v1.0.2"],
                cwd=root,
                env=env,
                text=True,
            )
            self.assertIn(
                "**Full changelog**: https://github.com/1jehuang/jcode/compare/v1.0.2...v1.0.3",
                output,
            )


if __name__ == "__main__":
    unittest.main()
