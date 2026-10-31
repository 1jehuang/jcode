#!/usr/bin/env python3
"""Preview publication safety and workflow regression tests, no network writes."""
import io
import json
import struct
import tarfile
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
import verify_preview_arm_runtime as arm_runtime

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
              patch.object(preview, "change_notes", return_value="## What changed\n\nTest change\n\n"),
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
        self.assertLess(notes.index("## What changed"), notes.index("## First-time preview setup"))
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


class PreviewChangeNotesTests(unittest.TestCase):
    def notes(self, releases, pages):
        with patch.object(preview, "gh", side_effect=[json.dumps(releases), json.dumps(pages)]) as gh:
            result = preview.change_notes(preview.REPOSITORY, SHA, TAG, gh)
        return result, gh.call_args_list[-1].args[-1]

    def commit(self, char, title):
        return {"sha": char * 40, "commit": {"message": title}}

    def test_first_preview_compares_latest_published_stable(self):
        notes, endpoint = self.notes([[{"tag_name": "v0.99.1", "published_at": "2026-10-22"},
                                      {"tag_name": "v0.98.0", "published_at": "2026-10-01"}]],
                                    [{"total_commits": 3, "commits": [
                                        self.commit("b", "feat: Add cloud handoff"),
                                        self.commit("c", "Fix readiness race"),
                                        self.commit("d", "Document migration")]}])
        self.assertIn("v0.99.1..." + SHA, endpoint)
        self.assertIn("3 commits since **v0.99.1**: 1 feature/addition commits, 1 fixes", notes)
        self.assertIn("**Highlights from commit titles**", notes)
        self.assertIn("- feat: Add cloud handoff", notes)
        self.assertIn("- Fix readiness race", notes)
        self.assertIn("Add cloud handoff", notes)
        self.assertIn("Fix readiness race", notes)

    def test_numeric_previous_preview_excludes_drafts_current_and_future(self):
        releases = [[{"tag_name": tag, "prerelease": True, "draft": draft}
                     for tag, draft in [("v1.0.0-preview.9", False),
                                        ("v1.0.0-preview.10", False),
                                        ("v1.0.0-preview.11", True),
                                        (TAG, False), ("v1.0.0-preview.13", False)]]]
        notes, endpoint = self.notes(releases, [{"total_commits": 0, "commits": []}])
        self.assertIn("preview.10...", endpoint)
        self.assertIn("No source changes", notes)

    def test_paginated_commits_preserved_and_titles_escaped(self):
        commit = self.commit("b", "Fix <script> [link](bad)\nNot a subject")
        notes, _ = self.notes([[{"tag_name": "v0.99.1"}]],
                              [{"total_commits": 2, "commits": [commit]},
                               {"commits": [commit, self.commit("c", "Add tests")]}])
        self.assertEqual(notes.count("bbbbbbbb]"), 1)
        self.assertIn("Add tests", notes)
        self.assertNotIn("<script>", notes)
        self.assertNotIn("Not a subject", notes)

    def test_large_change_list_is_bounded_and_links_omitted_commits(self):
        commits = [{"sha": f"{index:040x}", "commit": {"message": "Fix " + "x" * 500}}
                   for index in range(200)]
        notes, _ = self.notes([[{"tag_name": "v0.99.1"}]],
                              [{"total_commits": 200, "commits": commits}])
        self.assertLess(len(notes), 56000)
        self.assertIn("more commits in the [full comparison]", notes)
        self.assertLess(notes.index("000000c7)"), notes.index("000000c6)"))

    def test_missing_baseline_or_incomplete_comparison_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "baseline"):
            self.notes([[]], [])
        with self.assertRaisesRegex(ValueError, "Incomplete"):
            self.notes([[{"tag_name": "v0.99.1"}]],
                       [{"total_commits": 1, "commits": []}])


class ArmArchiveRuntimeTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.archive = Path(directory.name) / "arm.tar.gz"
        binary = bytearray(64)
        binary[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", binary, 16, 3, 183)
        self.binary = bytes(binary)
        self.make_archive()
        for attribute, value in (("system", "Linux"), ("machine", "aarch64")):
            mocked = patch.object(arm_runtime.platform, attribute, return_value=value)
            mocked.start()
            self.addCleanup(mocked.stop)

    def make_archive(self, name=arm_runtime.MEMBER, mode=0o755, kind=tarfile.REGTYPE, extra=False):
        with tarfile.open(self.archive, "w:gz") as package:
            member = tarfile.TarInfo(name)
            member.size = len(self.binary)
            member.mode = mode
            member.type = kind
            member.linkname = "/tmp/not-the-upload"
            package.addfile(member, io.BytesIO(self.binary))
            if extra:
                package.addfile(tarfile.TarInfo("unexpected"))

    def test_native_gate_executes_exact_archive_bytes_and_all_smokes(self):
        calls = []
        def execute(args, **kwargs):
            self.assertEqual(Path(args[0]).read_bytes(), self.binary)
            self.assertEqual(args[1:3], ["--no-update", "--no-selfdev"])
            self.assertTrue(kwargs["check"])
            self.assertEqual(kwargs["timeout"], 60)
            calls.append(args[3:])
            return subprocess.CompletedProcess(args, 0, stdout=f"jcode {TAG} (abc123)\n")
        with patch.object(arm_runtime.subprocess, "run", side_effect=execute):
            arm_runtime.verify(self.archive, TAG)
        self.assertEqual(calls, [["--version"]] + [
            ["cloud", command, "--help"] for command in
            ("receive", "credentials", "router-credentials", "cancel-return")])

    def test_non_native_runner_never_opens_or_executes_archive(self):
        with (patch.object(arm_runtime.platform, "machine", return_value="x86_64"),
              patch.object(arm_runtime.subprocess, "run") as execute,
              self.assertRaisesRegex(ValueError, "native ARM64")):
            arm_runtime.verify("missing.tar.gz", TAG)
        execute.assert_not_called()

    def test_unsafe_or_wrong_archive_never_executes(self):
        for change in ({"name": "../jcode-linux-aarch64"}, {"mode": 0o644},
                       {"kind": tarfile.SYMTYPE}, {"kind": tarfile.LNKTYPE}, {"extra": True}):
            with self.subTest(change=change):
                self.make_archive(**change)
                with (patch.object(arm_runtime.subprocess, "run") as execute,
                      self.assertRaises(ValueError)):
                    arm_runtime.verify(self.archive, TAG)
                execute.assert_not_called()
        for payload in (b"not an ARM executable", self.binary[:18] + b"\x3e\x00" + self.binary[20:]):
            self.binary = payload
            self.make_archive()
            with (patch.object(arm_runtime.subprocess, "run") as execute,
                  self.assertRaisesRegex(ValueError, "AArch64")):
                arm_runtime.verify(self.archive, TAG)
            execute.assert_not_called()

    def test_version_identity_is_not_a_prefix_match(self):
        for output in ("jcode v1.0.0-preview.123", "jcode v1.0.0-preview.12-extra", "jcode 0.99.1"):
            with (self.subTest(output=output),
                  patch.object(arm_runtime.subprocess, "run", return_value=
                               subprocess.CompletedProcess([], 0, stdout=output)) as execute,
                  self.assertRaisesRegex(ValueError, "wrong preview version")):
                arm_runtime.verify(self.archive, TAG)
            self.assertEqual(execute.call_count, 1)

    def test_version_and_smoke_failures_block_gate(self):
        for failure_at in range(5):
            calls = 0
            def execute(args, **kwargs):
                nonlocal calls
                calls += 1
                if calls == failure_at + 1:
                    raise subprocess.CalledProcessError(1, args)
                return subprocess.CompletedProcess(args, 0, stdout=f"jcode {TAG}\n")
            with (self.subTest(failure_at=failure_at),
                  patch.object(arm_runtime.subprocess, "run", side_effect=execute),
                  self.assertRaises(subprocess.CalledProcessError)):
                arm_runtime.verify(self.archive, TAG)
            self.assertEqual(calls, failure_at + 1)


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
        self.assertEqual(workflow.count('ref: ${{ github.sha }}'), 4)
        self.assertIn('grep -F "$JCODE_BUILD_SEMVER"', workflow)
        self.assertIn('bash scripts/build_preview_linux_hosted.sh', workflow)
        self.assertIn('bash scripts/install_justrust_hosted_ci.sh', workflow)
        self.assertIn('toolchain: 1.99.0', workflow)
        self.assertIn('bash scripts/package_preview_linux_hosted.sh', workflow)
        self.assertLess(workflow.index('bash scripts/build_preview_linux_hosted.sh'),
                        workflow.index('bash scripts/package_preview_linux_hosted.sh'))
        self.assertIn('JCODE_API_KEY: ${{ secrets.JUSTRUST_API_KEY }}', workflow)
        self.assertEqual(workflow.count('            hosted: true'), 2)
        self.assertIn('CARGO_PROFILE_RELEASE_OPT_LEVEL: "3"', workflow)
        self.assertIn('CARGO_INCREMENTAL: "0"', workflow)
        self.assertNotIn('continue-on-error:', workflow)
        for forbidden in ('quick-release', 'npm ', 'brew ', 'DEPLOY_KEY', 'latest.json', 'push origin', 'secrets.PAT'):
            self.assertNotIn(forbidden, workflow)
        # Only publication writes contents, no PAT capable of recursive tag jobs.
        self.assertEqual(workflow.count('contents: write'), 1)
        self.assertIn('GH_TOKEN: ${{ github.token }}', workflow)
        self.assertIn('needs: publish', workflow)
        self.assertIn('gh workflow run discord-release.yml', workflow)
        self.assertNotIn('gh workflow run release.yml', workflow)

    def test_hosted_linux_credentials_are_scoped_to_build_step(self):
        workflow = (ROOT / ".github/workflows/preview.yml").read_text()
        hosted = workflow.split("      - name: Build Linux preview on hosted justrust\n", 1)[1]
        hosted = hosted.split("      - name:", 1)[0]
        self.assertIn("if: matrix.hosted == true", hosted)
        self.assertIn("secrets.JUSTRUST_API_KEY", hosted)
        self.assertEqual(workflow.count("secrets.JUSTRUST_API_KEY"), 1)
        native = workflow.split("      - name: Build native preview\n", 1)[1]
        native = native.split("      - name:", 1)[0]
        self.assertIn("matrix.hosted != true", native)
        self.assertNotIn("aarch64-unknown-linux-gnu", native)
        self.assertNotIn("secrets.", native)
        cache = workflow.split("      - uses: Swatinem/rust-cache@v2\n", 1)[1]
        self.assertTrue(cache.startswith("        if: matrix.hosted != true\n"))

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

    def test_windows_build_uses_native_shell_and_propagates_failure(self):
        workflow = (ROOT / ".github/workflows/preview.yml").read_text()
        native = workflow.split("      - name: Build native preview\n", 1)[1]
        self.assertIn("if: matrix.hosted != true && runner.os != 'Windows'",
                      native.split("        run: |", 1)[0])
        windows = workflow.split(
            "      - name: Build Windows preview with MSVC environment\n", 1)[1]
        windows = windows.split("      - name:", 1)[0]
        self.assertIn("if: runner.os == 'Windows'", windows)
        self.assertIn("shell: pwsh", windows)
        self.assertIn("$extraArgs = @()", windows)
        self.assertIn("$env:EXTRA_ARGS -split '\\s+'", windows)
        self.assertIn("--target $env:TARGET -p jcode --bin jcode @extraArgs", windows)
        self.assertIn("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }", windows)

    def test_stable_pipeline_excludes_preview_at_trigger_and_job(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("- '!v*-preview.*'", workflow)
        self.assertIn("!contains(inputs.tag || github.ref_name, '-preview.')", workflow)

    def test_arm_compile_and_native_gate_use_same_uploaded_archive(self):
        workflow = (ROOT / ".github/workflows/preview.yml").read_text()
        build = workflow.split("  build:\n", 1)[1].split("  native-arm-runtime:\n", 1)[0]
        self.assertIn("- os: ubuntu-24.04\n            target: aarch64-unknown-linux-gnu\n"
                      "            artifact: jcode-linux-aarch64\n            hosted: true", build)
        self.assertNotIn("ubuntu-24.04-arm", build)
        self.assertEqual(build.count("            artifact: jcode-"), 6)
        self.assertIn("name: preview-${{ matrix.artifact }}", build)
        self.assertIn("path: dist/${{ matrix.artifact }}.tar.gz", build)
        gate = workflow.split("  native-arm-runtime:\n", 1)[1].split("  publish:\n", 1)[0]
        self.assertIn("needs: build", gate)
        self.assertIn("runs-on: ubuntu-24.04-arm", gate)
        self.assertIn("uses: actions/download-artifact@v4", gate)
        self.assertIn("name: preview-jcode-linux-aarch64", gate)
        self.assertIn("python3 scripts/verify_preview_arm_runtime.py dist/jcode-linux-aarch64.tar.gz", gate)
        for forbidden in ("secrets.", "justrust", "upload-artifact", "tar czf", "continue-on-error", "if:"):
            self.assertNotIn(forbidden, gate)
        publish = workflow.split("  publish:\n", 1)[1].split("  announce:\n", 1)[0]
        self.assertIn("needs: [quiet-and-ci, build, native-arm-runtime]", publish)
        self.assertNotIn("always()", publish)
        self.assertIn("pattern: preview-jcode-*", publish)

    def test_arm_packaging_never_executes_cross_binary(self):
        workflow = (ROOT / ".github/workflows/preview.yml").read_text()
        package = workflow.split("      - name: Package native binary\n", 1)[1]
        body = textwrap.dedent(package.split("        run: |\n", 1)[1]
                               .split("      - name:", 1)[0])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target/aarch64-unknown-linux-gnu/release"
            target.mkdir(parents=True)
            binary = target / "jcode"
            binary.write_text("#!/bin/sh\necho executed > should-not-execute\nexit 99\n")
            binary.chmod(0o755)
            subprocess.run(["bash", "-c", body], cwd=root, check=True,
                           env={**os.environ, "TARGET": "aarch64-unknown-linux-gnu",
                                "ARTIFACT": arm_runtime.MEMBER, "RUNNER_OS": "Linux"})
            self.assertFalse((root / "should-not-execute").exists())
            with tarfile.open(root / "dist/jcode-linux-aarch64.tar.gz") as archive:
                self.assertEqual(archive.getnames(), [arm_runtime.MEMBER])
                self.assertEqual(archive.extractfile(arm_runtime.MEMBER).read(), binary.read_bytes())

    def test_ci_runs_preview_tests(self):
        self.assertIn('python3 scripts/test_preview_release.py',
                      (ROOT / '.github/workflows/ci.yml').read_text())


if __name__ == "__main__":
    unittest.main()
