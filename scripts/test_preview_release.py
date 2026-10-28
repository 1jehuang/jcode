#!/usr/bin/env python3
"""Preview publication safety and workflow regression tests, no network writes."""
import json
import os
import subprocess
import textwrap
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import preview_release as preview

ROOT = Path(__file__).resolve().parents[1]
SHA = "a" * 40
TAG = "v1.0.0-preview.12"


class PreviewPublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        for name in preview.ASSETS:
            (self.directory / name).write_bytes(b"test binary archive")
        self.calls = []
        self.stale = False
        self.incomplete = False
        self.existing = False
        self.corrupt_asset = None
        self.changed_tag = False

    def fake_gh(self, *args):
        self.calls.append(args)
        if args[0] == "api":
            if "--slurp" in args:
                return json.dumps([[{"ref": f"refs/tags/{TAG}"}]] if self.existing else [[]])
            if "-X" not in args:
                return json.dumps({"object": {"sha": "b" * 40 if self.changed_tag else SHA}})
            return "{}"
        if args[:2] == ("release", "view"):
            paths = list(self.directory.iterdir())
            assets = [{"name": p.name, "size": p.stat().st_size, "state": "uploaded"} for p in paths]
            if self.incomplete:
                assets.pop()
            if self.corrupt_asset == "size":
                assets[0]["size"] += 1
            elif self.corrupt_asset == "state":
                assets[0]["state"] = "new"
            elif self.corrupt_asset == "duplicate":
                assets[-1] = assets[0]
            return json.dumps({"isDraft": True, "isPrerelease": True, "assets": assets})
        return ""

    def head(self, repository, sha):
        self.calls.append(("head", repository, sha))
        if self.stale and sum(call[0] == "head" for call in self.calls) == 2:
            raise ValueError("master advanced")

    def publish(self, ci_error=None):
        with (patch.object(preview, "gh", side_effect=self.fake_gh),
              patch.object(preview, "require_head", side_effect=self.head),
              patch.object(preview, "authorize", side_effect=ci_error) as authorize):
            preview.publish(preview.REPOSITORY, SHA, TAG, self.directory)
        authorize.assert_called_once_with(preview.REPOSITORY, SHA, timeout=0)

    def test_complete_assets_publish_only_as_nonlatest_prerelease(self):
        self.publish()
        create = next(call for call in self.calls if call[:2] == ("release", "create"))
        self.assertIn("--draft", create)
        self.assertIn("--prerelease", create)
        self.assertIn("--latest=false", create)
        self.assertIn("--verify-tag", create)
        notes = create[create.index("--notes") + 1]
        for instruction in ("Existing stable versions do not support", "SHA256SUMS",
                            "extract the entire archive", "full path", "update --channel preview",
                            "update --channel stable", "v1.0.0"):
            self.assertIn(instruction, notes)
        self.assertEqual(self.calls[-2], ("head", preview.REPOSITORY, SHA))
        self.assertEqual(self.calls[-1][:3], ("release", "edit", TAG))
        self.assertIn("--latest=false", self.calls[-1])
        self.assertIn("--prerelease", self.calls[-1])
        self.assertIn("--draft=false", self.calls[-1])
        checksums = (self.directory / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(checksums), len(preview.ASSETS))
        self.assertTrue(all(len(line.split()[0]) == 64 for line in checksums))
        self.assertFalse(any("workflow" in call for call in self.calls))

    def test_missing_or_empty_asset_never_creates_tag(self):
        for mode in ("missing", "empty", "extra"):
            with self.subTest(mode=mode):
                asset = self.directory / preview.ASSETS[0]
                if mode == "missing":
                    asset.unlink()
                elif mode == "empty":
                    asset.write_bytes(b"")
                else:
                    asset.write_bytes(b"binary")
                    (self.directory / "unexpected.exe").write_bytes(b"binary")
                with self.assertRaises(ValueError):
                    self.publish()
                self.assertEqual(self.calls, [])

    def test_failed_ci_never_creates_tag(self):
        with self.assertRaisesRegex(RuntimeError, "failed CI"):
            self.publish(RuntimeError("failed CI"))
        self.assertEqual(self.calls, [])

    def test_stale_head_leaves_draft_unpublished(self):
        self.stale = True
        with self.assertRaisesRegex(ValueError, "master advanced"):
            self.publish()
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))

    def test_incomplete_remote_assets_leave_draft_unpublished(self):
        self.incomplete = True
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.publish()
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))

    def test_mutated_tag_leaves_draft_unpublished(self):
        self.changed_tag = True
        with self.assertRaisesRegex(ValueError, "tag changed"):
            self.publish()
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))

    def test_wrong_remote_size_state_or_duplicate_is_rejected(self):
        for corruption in ("size", "state", "duplicate"):
            with self.subTest(corruption=corruption):
                self.corrupt_asset = corruption
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    self.publish()
                self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))
                (self.directory / "SHA256SUMS").unlink()

    def test_existing_tag_is_never_overwritten(self):
        self.existing = True
        with self.assertRaisesRegex(ValueError, "already exists"):
            self.publish()
        self.assertFalse(any("POST" in call for call in self.calls))

    def test_only_canonical_repository_preview_tag_and_full_sha(self):
        for repo, sha, tag in (("fork/jcode", SHA, TAG), (preview.REPOSITORY, "abc", TAG),
                               (preview.REPOSITORY, SHA, "v1.0.0"),
                               (preview.REPOSITORY, SHA, "v1.0.0-preview.0"),
                               (preview.REPOSITORY, SHA, "v1.0.0-preview.1;evil")):
            with self.subTest(repo=repo, sha=sha, tag=tag), self.assertRaises(ValueError):
                preview.validate(repo, sha, tag)

    def test_stable_v1_tag_stops_preview_without_confusing_preview_tags(self):
        for refs, stopped in ((["refs/tags/v1.0.0"], True),
                              (["refs/tags/v1.0.0-preview.3"], False), ([], False)):
            response = [[{"ref": ref} for ref in refs]]
            with self.subTest(refs=refs), patch.object(preview, "gh", return_value=json.dumps(response)):
                if stopped:
                    with self.assertRaisesRegex(ValueError, "preview publishing has ended"):
                        preview.require_preview_window(preview.REPOSITORY)
                else:
                    preview.require_preview_window(preview.REPOSITORY)

    def test_v1_sunset_during_staging_leaves_draft_unpublished(self):
        with (patch.object(preview, "require_preview_window",
                           side_effect=[None, ValueError("preview publishing has ended")]),
              self.assertRaisesRegex(ValueError, "preview publishing has ended")):
            self.publish()
        self.assertFalse(any(call[:2] == ("release", "edit") for call in self.calls))

    def test_stable_v1_tag_before_build_prevents_any_publication(self):
        with (patch.object(preview, "require_preview_window",
                           side_effect=ValueError("preview publishing has ended")),
              self.assertRaisesRegex(ValueError, "preview publishing has ended")):
            self.publish()
        self.assertEqual(self.calls, [])

    def test_head_mismatch_fails_closed(self):
        with patch.object(preview, "api", return_value={"object": {"sha": "b" * 40}}):
            with self.assertRaisesRegex(ValueError, "master advanced"):
                preview.require_head(preview.REPOSITORY, SHA)


class PreviewWorkflowTests(unittest.TestCase):
    def test_workflow_is_opt_in_debounced_exact_sha_and_nonrecursive(self):
        workflow = (ROOT / ".github/workflows/preview.yml").read_text()
        triggers = workflow.split("on:\n", 1)[1].split("concurrency:", 1)[0]
        self.assertIn("branches: [master]", triggers)
        self.assertNotIn("tags:", triggers)
        self.assertNotIn("workflow_run:", triggers)
        self.assertNotIn("release:", triggers)
        self.assertIn("cancel-in-progress: true", workflow)
        self.assertIn("group: v1-preview-master", workflow)
        self.assertIn("vars.JCODE_V1_PREVIEW_ENABLED != 'false'", workflow)
        self.assertLess(workflow.index("run: sleep 900"), workflow.index("check_release_ci.py"))
        self.assertIn('check_release_ci.py "$GITHUB_SHA"', workflow)
        self.assertIn('JCODE_BUILD_SEMVER: v1.0.0-preview.${{ github.run_number }}', workflow)
        self.assertEqual(workflow.count('ref: ${{ github.sha }}'), 3)
        self.assertIn('grep -F "$JCODE_BUILD_SEMVER"', workflow)
        self.assertIn('bash scripts/build_linux_compat.sh dist', workflow)
        self.assertIn('RUSTUP_TOOLCHAIN=nightly justrust build --local --locked -Z build-std=std,panic_abort', workflow)
        self.assertNotIn('continue-on-error:', workflow)
        for forbidden in ('quick-release', 'npm ', 'brew ', 'DEPLOY_KEY', 'latest.json', 'push origin', 'secrets.PAT'):
            self.assertNotIn(forbidden, workflow)
        # Only publication writes contents, no PAT capable of recursive tag jobs.
        self.assertEqual(workflow.count('contents: write'), 1)
        self.assertIn('GH_TOKEN: ${{ github.token }}', workflow)
        self.assertIn('needs: publish', workflow)
        self.assertIn('gh workflow run discord-release.yml', workflow)
        self.assertNotIn('gh workflow run release.yml', workflow)

    def test_native_build_preserves_empty_and_nonempty_options(self):
        workflow = (ROOT / ".github/workflows/preview.yml").read_text()
        native = workflow.split("      - name: Build native preview\n", 1)[1]
        body = textwrap.dedent(native.split("        run: |\n", 1)[1]
                               .split("      - name:", 1)[0])
        # Bash 3.2 treats an empty array as unset under nounset. This path must
        # use the portable positional-parameter expansion for an empty list.
        self.assertIn('bin jcode "$@"', body)
        for extra, expected in (("", []),
                                ("--no-default-features --features pdf",
                                 ["--no-default-features", "--features", "pdf"])):
            with self.subTest(extra=extra):
                result = subprocess.run(
                    ["bash", "-c", 'justrust() { printf "%s\\n" "$@"; }\n' + body],
                    env={**os.environ, "TARGET": "aarch64-apple-darwin",
                         "EXTRA_ARGS": extra},
                    text=True, capture_output=True, check=True)
                self.assertEqual(result.stdout.splitlines(),
                                 ["build", "--local", "--locked", "--release",
                                  "--target", "aarch64-apple-darwin", "-p",
                                  "jcode", "--bin", "jcode"] + expected)

    def test_stable_pipeline_excludes_preview_at_trigger_and_job(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("- '!v*-preview.*'", workflow)
        self.assertIn("!contains(inputs.tag || github.ref_name, '-preview.')", workflow)

    def test_ci_runs_preview_tests(self):
        self.assertIn('python3 scripts/test_preview_release.py',
                      (ROOT / '.github/workflows/ci.yml').read_text())


if __name__ == "__main__":
    unittest.main()
