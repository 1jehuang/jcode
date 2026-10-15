#!/usr/bin/env python3
"""Exercise the real release entry point with isolated Git remotes and fake CI.

No GitHub writes, Rust builds, or network requests are made.
Run: python3 scripts/test_quick_release.py
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TAG = "v1.2.3"
MANDATORY_JOBS = (
    "Quality Guardrails",
    "Build & Test (ubuntu-latest)",
    "Build & Test (macos-latest)",
    "Build & Test (windows-latest)",
    "Windows ARM64 Release Configuration",
)
FAKE_GH = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import sys

args = sys.argv[1:]
config = json.loads(Path(os.environ["TEST_CI_CONFIG"]).read_text())
scenario = config["scenario"]
with open(os.environ["TEST_GH_LOG"], "a") as log:
    log.write(json.dumps(args) + "\n")
if args[:2] == ["repo", "view"]:
    if scenario == "repo_error":
        sys.exit(1)
    print("owner/repo")
    sys.exit(0)
if args[0] == "release":
    if args[1] == "view" and "--json" in args:
        print("false" if scenario == "public_release" else "true")
    sys.exit(0)
if args[0] != "api":
    sys.exit("Unexpected gh command: " + repr(args))
if scenario == "api_error":
    sys.exit(1)
endpoint = args[1]
run = dict(id=100, run_number=10, run_attempt=1, head_sha=config["sha"],
           event="push", head_branch="master", path=".github/workflows/ci.yml",
           status="completed", conclusion="success")
if scenario == "failure":
    run["conclusion"] = "failure"
elif scenario == "pending":
    run.update(status="in_progress", conclusion=None)
elif scenario == "other_sha":
    run["head_sha"] = "b" * 40
elif scenario == "pull_request":
    run["event"] = "pull_request"
if "/workflows/ci.yml/runs?" in endpoint:
    print(json.dumps([{"workflow_runs": [] if scenario == "missing" else [run]}]))
elif "/jobs?" in endpoint:
    jobs = [dict(name=name, status="completed", conclusion="success")
            for name in config["jobs"]]
    if scenario == "missing_job":
        jobs.pop()
    elif scenario == "failed_job":
        jobs[-1]["conclusion"] = "failure"
    print(json.dumps([{"jobs": jobs}]))
else:
    marker = Path(os.environ["TEST_CONFIRMATION_MARKER"])
    if marker.exists():
        if scenario == "head_changed":
            subprocess.run(["git", "commit", "--allow-empty", "-m", "Concurrent change"],
                           check=True, stdout=subprocess.DEVNULL)
        elif scenario == "local_tag_changed":
            subprocess.run(["git", "tag", "-f", "v1.2.3", "HEAD^"],
                           check=True, stdout=subprocess.DEVNULL)
        elif scenario == "rerun":
            run["run_attempt"] = 2
        elif scenario == "dirty_during_ci":
            with open("Cargo.toml", "a") as manifest:
                manifest.write("# Concurrent source change\n")
    marker.touch()
    print(json.dumps(run))
'''


class QuickReleaseTests(unittest.TestCase):
    def setUp(self):
        scratch = os.environ.get("JCODE_SCRATCH_DIR")
        self.temp = tempfile.TemporaryDirectory(prefix="quick-release-test-", dir=scratch)
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.repo = self.base / "source"
        self.origin = self.base / "origin.git"
        self.repo.mkdir()
        (self.repo / "scripts").mkdir()
        for name in ("quick-release.sh", "check_release_ci.py"):
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        (self.repo / "Cargo.toml").write_text('[package]\nname = "release-fixture"\nversion = "1.2.3"\n')
        self.git("init", "-b", "master")
        self.git("config", "user.name", "Release Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        self.git("add", "scripts", "Cargo.toml")
        self.git("commit", "-m", "Initial fixture")
        self.git("commit", "--allow-empty", "-m", "Release metadata")
        self.sha = self.git("rev-parse", "HEAD").stdout.strip()
        subprocess.run(["git", "init", "--bare", str(self.origin)], check=True,
                       capture_output=True, text=True)
        self.git("remote", "add", "origin", str(self.origin))
        self.git("push", "origin", "master")
        fake_bin = self.base / "bin"
        fake_bin.mkdir()
        fake_gh = fake_bin / "gh"
        fake_gh.write_text(FAKE_GH)
        fake_gh.chmod(0o755)
        self.config = self.base / "ci.json"
        self.log = self.base / "gh.jsonl"
        self.env = dict(os.environ, PATH=f"{fake_bin}:{os.environ['PATH']}",
                        TEST_CI_CONFIG=str(self.config), TEST_GH_LOG=str(self.log),
                        TEST_CONFIRMATION_MARKER=str(self.base / "confirmed"))

    def git(self, *args, check=True):
        return subprocess.run(["git", *args], cwd=self.repo, check=check,
                              capture_output=True, text=True)

    def release(self, scenario="success", *options, mode="--remote"):
        Path(self.env["TEST_CONFIRMATION_MARKER"]).unlink(missing_ok=True)
        self.config.write_text(json.dumps(dict(scenario=scenario, sha=self.sha,
                                               jobs=MANDATORY_JOBS)))
        return subprocess.run(["bash", "scripts/quick-release.sh", *([mode] if mode else []), *options, TAG],
                              cwd=self.repo, env=self.env, capture_output=True,
                              text=True, timeout=10)

    def prepare_standard(self):
        """Stub only binary builders/signing, retaining the actual staging path."""
        self.env["HOME"] = str(self.base)
        fake_bin = self.base / "bin"
        programs = {
            "cargo": "mkdir -p target/aarch64-apple-darwin/release\nprintf fixture > target/aarch64-apple-darwin/release/jcode\n",
            "docker": "exit 0\n",
            "rcodesign": "exit 0\n",
            "file": "echo 'ELF 64-bit Mach-O 64-bit'\n",
            "aarch64-apple-darwin23.5-clang": "exit 0\n",
        }
        for name, body in programs.items():
            program = fake_bin / name
            program.write_text("#!/bin/sh\nset -eu\n" + body)
            program.chmod(0o755)
        (self.repo / ".gitignore").write_text("target/\n")
        builder = self.repo / "scripts/build_linux_compat.sh"
        builder.write_text('#!/bin/sh\nset -eu\nprintf "#!/bin/sh\\n" > "$1/jcode-linux-x86_64"\nprintf fixture > "$1/jcode-linux-x86_64.bin"\n(cd "$1" && tar czf jcode-linux-x86_64.tar.gz jcode-linux-x86_64 jcode-linux-x86_64.bin)\n')
        builder.chmod(0o755)
        self.git("add", ".gitignore", "scripts/build_linux_compat.sh")
        self.git("commit", "-m", "Local build fixtures")
        self.sha = self.git("rev-parse", "HEAD").stdout.strip()

    def assert_no_tag(self):
        self.assertNotEqual(self.git("show-ref", "--verify", f"refs/tags/{TAG}",
                                     check=False).returncode, 0)
        result = self.git("ls-remote", "--tags", "origin", f"refs/tags/{TAG}")
        self.assertEqual(result.stdout, "")

    def test_green_exact_sha_pushes_verified_annotated_tag(self):
        result = self.release()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", f"refs/tags/{TAG}^{{commit}}").stdout.strip(), self.sha)
        self.assertEqual(self.git("cat-file", "-t", f"refs/tags/{TAG}").stdout.strip(), "tag")
        self.assertIn(self.sha, self.git("ls-remote", "--tags", "origin", f"refs/tags/{TAG}^{{}}").stdout)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual(calls[0], ["repo", "view", str(self.origin), "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
        self.assertIn(f"head_sha={self.sha}&event=push", calls[1][1])
        self.assertIn("/attempts/1/jobs?", calls[3][1])
        self.assertIn("AUTHORIZED", result.stdout)

    def test_non_green_or_incorrect_ci_never_creates_or_pushes_tag(self):
        for scenario in ("missing", "pending", "failure", "other_sha", "pull_request",
                         "missing_job", "failed_job", "api_error", "repo_error", "rerun"):
            with self.subTest(scenario=scenario):
                result = self.release(scenario)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assert_no_tag()

    def test_failed_ci_does_not_push_existing_local_tag(self):
        self.git("tag", "-a", TAG, "-m", TAG)
        result = self.release("failure")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git("ls-remote", "--tags", "origin", f"refs/tags/{TAG}").stdout, "")

    def test_wrong_committed_package_version_never_tags(self):
        manifest = self.repo / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace("1.2.3", "1.2.2"))
        self.git("add", "Cargo.toml")
        self.git("commit", "-m", "Wrong version")
        self.sha = self.git("rev-parse", "HEAD").stdout.strip()
        result = self.release()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match requested tag", result.stderr)
        self.assert_no_tag()
        self.assertFalse(self.log.exists())

    def test_remote_checks_committed_version_not_dirty_manifest(self):
        manifest = self.repo / "Cargo.toml"
        manifest.write_text(manifest.read_text().replace("1.2.3", "9.9.9"))
        result = self.release()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", f"refs/tags/{TAG}^{{commit}}").stdout.strip(), self.sha)

    def test_existing_remote_tag_still_requires_ci(self):
        self.git("tag", "-a", TAG, "-m", TAG)
        self.git("push", "origin", f"refs/tags/{TAG}")
        result = self.release("failure")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("release blocked", result.stderr)

    def test_existing_green_tag_is_idempotent(self):
        self.git("tag", "-a", TAG, "-m", TAG)
        self.git("push", "origin", f"refs/tags/{TAG}")
        result = self.release()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already exists at the CI-verified commit", result.stdout)

    def test_conflicting_local_or_remote_tag_rejected(self):
        self.git("tag", "-a", TAG, "HEAD^", "-m", TAG)
        result = self.release()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("different commit", result.stderr)
        self.git("push", "origin", f"refs/tags/{TAG}")
        self.git("tag", "-d", TAG)
        result = self.release()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Remote tag", result.stderr)

    def test_remote_lookup_error_fails_closed(self):
        self.git("remote", "set-url", "origin", str(self.base / "missing.git"))
        result = self.release()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotEqual(self.git("show-ref", "--verify", f"refs/tags/{TAG}", check=False).returncode, 0)
        self.assertFalse(self.log.exists())

    def test_head_change_during_ci_never_creates_or_pushes_tag(self):
        result = self.release("head_changed")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HEAD changed during CI preflight", result.stderr)
        self.assert_no_tag()

    def test_local_tag_change_during_ci_is_not_pushed(self):
        self.git("tag", "-a", TAG, "-m", TAG)
        result = self.release("local_tag_changed")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Local tag changed during CI preflight", result.stderr)
        self.assertEqual(self.git("ls-remote", "--tags", "origin", f"refs/tags/{TAG}").stdout, "")

    def test_same_named_branch_does_not_change_push_target(self):
        self.git("branch", TAG, "HEAD^")
        result = self.release()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("ls-remote", "--heads", "origin", f"refs/heads/{TAG}").stdout, "")

    def test_remote_dry_run_rejected_before_ci_or_tag(self):
        result = self.release("success", "--dry-run")
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_tag()
        self.assertFalse(self.log.exists())

    def test_all_packaging_modes_stage_only_and_use_shared_gate(self):
        script = (ROOT / "scripts/quick-release.sh").read_text()
        self.assertNotIn("--draft=false", script)
        self.assertEqual(script.count("    tag_and_push\n"), 3)
        self.assertIn("\ntag_and_push\n", script)
        self.assertIn('gh release view "$VERSION" --json isDraft --jq .isDraft', script)
        self.assertIn("Refusing to replace published assets", script)

    def test_standard_green_release_stages_without_publishing(self):
        self.prepare_standard()
        result = self.release(mode="")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertTrue(any(call[:2] == ["release", "upload"] for call in calls))
        self.assertFalse(any("--draft=false" in call for call in calls))
        self.assertIn("every required platform asset", result.stdout)

    def test_standard_dirty_source_rejected_before_tagging(self):
        self.prepare_standard()
        with (self.repo / "Cargo.toml").open("a") as manifest:
            manifest.write("# Local edit\n")
        result = self.release(mode="")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires a clean working tree", result.stderr)
        self.assert_no_tag()

    def test_standard_dirty_source_during_ci_never_tags_or_uploads(self):
        self.prepare_standard()
        result = self.release("dirty_during_ci", mode="")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("working tree changed during CI preflight", result.stderr)
        self.assert_no_tag()
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertFalse(any(call[0] == "release" for call in calls))

    def test_standard_staging_never_overwrites_public_release(self):
        self.prepare_standard()
        result = self.release("public_release", mode="")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Refusing to replace published assets", result.stderr)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertFalse(any(call[:2] in (["release", "upload"], ["release", "edit"]) for call in calls))


if __name__ == "__main__":
    unittest.main()
