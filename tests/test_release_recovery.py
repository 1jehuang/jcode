"""Exercise recovery against real tagged source in isolated local clones."""
import hashlib
import itertools
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/apply_release_recovery.py"
SOURCE = "crates/jcode-companion/src/server.rs"
FIX = "d4f4c8c8d25c9f8e062773683adcc0c19cf26144"


class WorkflowGraphTests(unittest.TestCase):
    """Check real job predicates, including the implicit ancestor success gate.

    This is a deliberately restricted model of the status predicates used here,
    not a GitHub Actions emulator. Matrix results are terminal aggregate results.
    """

    def setUp(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.jobs = dict(re.findall(
            r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)",
            workflow.split("\njobs:\n", 1)[1], re.M | re.S))
        self.needs = {}
        self.conditions = {}
        for name, body in self.jobs.items():
            needs = re.search(r"^    needs: (.+)$", body, re.M)
            self.needs[name] = (needs.group(1).strip("[]").split(", ")
                                if needs else [])
            self.conditions[name] = re.search(r"^    if: (.+)$", body, re.M).group(1)

    def ancestors(self, job):
        return set(self.needs[job]).union(
            *(self.ancestors(parent) for parent in self.needs[job]))

    def eligible(self, job, results):
        expression = self.conditions[job].removeprefix("${{ ").removesuffix(" }}")
        # Without a status function, Actions implicitly requires success(),
        # including transitive needs. This is what broke Windows publication.
        if "always()" not in expression and any(
                results[parent] != "success" for parent in self.ancestors(job)):
            return False
        expression = expression.replace("always()", "True")
        expression = re.sub(r"needs\.([\w-]+)\.result",
                            lambda match: repr(results[match.group(1)]), expression)
        expression = expression.replace("&&", "and").replace("||", "or")
        # Reject unexpected syntax rather than executing arbitrary workflow text.
        residue = re.sub(r"'[^']*'|True|and|or|==|[()\s]", "", expression)
        self.assertEqual(residue, "", expression)
        return eval(expression, {"__builtins__": {}})

    def test_complete_dependency_graph(self):
        self.assertEqual(self.needs, {
            "create-release": [],
            "recovery-preflight": ["create-release"],
            "build-linux-macos": ["create-release", "recovery-preflight"],
            "build-windows": ["create-release", "recovery-preflight"],
            "publish-windows": ["create-release", "build-windows"],
            "build-freebsd": ["create-release", "recovery-preflight"],
            "release": ["create-release", "build-linux-macos", "publish-windows", "build-freebsd"],
        })
        self.assertIn("github.repository == '1jehuang/jcode'", self.conditions["create-release"])
        self.assertEqual(self.conditions["recovery-preflight"],
                         "github.event_name == 'workflow_dispatch' && inputs.recovery == 'windows-listener'")

    def test_all_terminal_ancestor_results(self):
        for job in ("build-linux-macos", "build-windows", "build-freebsd",
                    "publish-windows", "release"):
            ancestors = sorted(self.ancestors(job))
            for statuses in itertools.product(("success", "skipped", "failure", "cancelled"),
                                              repeat=len(ancestors)):
                results = dict(zip(ancestors, statuses))
                expected = results["create-release"] == "success"
                if job.startswith("build-"):
                    expected &= results["recovery-preflight"] in ("success", "skipped")
                elif job == "publish-windows":
                    expected &= results["build-windows"] == "success"
                # Finalization must run after platform failures to diagnose the
                # incomplete asset set, not silently skip or publish partial sets.
                with self.subTest(job=job, results=results):
                    self.assertEqual(self.eligible(job, results), expected)

    def test_missing_status_override_reproduces_skipped_ancestor_bug(self):
        results = {"create-release": "success", "build-windows": "success",
                   "recovery-preflight": "skipped"}
        self.assertTrue(self.eligible("publish-windows", results))
        self.conditions["publish-windows"] = self.conditions["publish-windows"].replace(
            "always() && ", "")
        self.assertFalse(self.eligible("publish-windows", results))
        results["recovery-preflight"] = "success"
        self.assertTrue(self.eligible("publish-windows", results))

    def test_validation_triggers_and_executes_release_checks(self):
        workflow = (ROOT / ".github/workflows/release-recovery.yml").read_text()
        for event in ("pull_request", "push"):
            section = re.search(rf"^  {event}:\n(.*?)(?=^  \w+:)", workflow, re.M | re.S).group(1)
            for path in (".github/workflows/release.yml", ".github/workflows/release-recovery.yml",
                         ".github/workflows/windows-companion.yml", ".github/scripts/verify_windows_install.ps1",
                         "scripts/*release*", "scripts/test_*release*.py", "tests/*release*.py",
                         "scripts/install.sh", "scripts/install_justrust_ci.sh", "scripts/build_linux_compat.sh"):
                self.assertIn(f"- '{path}'", section, (event, path))
        self.assertIn('fetch-depth: 0', workflow)
        self.assertIn("python3 -m unittest discover -s tests -p '*release*.py'", workflow)
        self.assertIn("python3 -m unittest discover -s scripts -p 'test_*release*.py'", workflow)
        self.assertIn("bash scripts/test_install_release_metadata.sh", workflow)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name) / "repo"
        subprocess.run(["git", "clone", "--quiet", "--shared", "--no-checkout", str(ROOT), str(self.repo)], check=True)
        self.workflow_sha = self.git("rev-parse", "HEAD")
        if self._testMethodName == "test_windows_crlf_checkout":
            self.git("config", "core.autocrlf", "true")
        self.git("checkout", "--quiet", "--detach", "v0.96.0")
        self.tag_sha = self.git("rev-parse", "HEAD")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.PIPE).decode().strip()

    def run_repair(self, tag="v0.96.0", repair="windows-listener", workflow_sha=None):
        return subprocess.run(["python3", str(SCRIPT), tag, repair, workflow_sha or self.workflow_sha],
                              cwd=self.repo, capture_output=True, text=True)

    def test_real_tags_preserve_identity_and_verify_exact_repair(self):
        for tag in ("v0.96.0", "v0.97.0"):
            with self.subTest(tag=tag):
                # Fresh checkout clears only this disposable clone's repair.
                self.git("checkout", "--quiet", "--force", "--detach", tag)
                before = self.git("rev-parse", "HEAD")
                result = self.run_repair(tag=tag)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.git("rev-parse", "HEAD"), before)
                self.assertEqual(self.git("rev-parse", f"refs/tags/{tag}^{{commit}}"), before)
                changed = set(self.git("diff", "--name-only").splitlines())
                self.assertIn(SOURCE, changed)
                self.assertLessEqual(changed, {SOURCE, "scripts/build_linux_compat.sh", "scripts/install_justrust_ci.sh"})
                data = json.loads((self.repo / "release-provenance.json").read_text())
                self.assertEqual(data["fix_commit"], FIX)
                self.assertEqual(data["tag_commit"], before)
                self.assertEqual(data["workflow_commit"], self.workflow_sha)
                self.assertEqual(data["repaired_sha256"], hashlib.sha256((self.repo / SOURCE).read_bytes()).hexdigest())
                for helper in ("scripts/install_justrust_ci.sh", "scripts/build_linux_compat.sh"):
                    self.assertEqual(data["release_helpers"][helper], hashlib.sha256((self.repo / helper).read_bytes()).hexdigest())
                    self.assertTrue(os.access(self.repo / helper, os.X_OK))
                self.assertNotEqual(self.run_repair(tag=tag).returncode, 0)

    def test_none_is_unpatched_source(self):
        result = self.run_repair(repair="none")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("diff", "--name-only", "--", SOURCE), "")
        self.assertEqual(json.loads((self.repo / "release-provenance.json").read_text())["repair"], "none")

    def test_none_with_external_provenance_stays_clean(self):
        self.git("checkout", "--quiet", "--detach", "v0.97.0")
        output = Path(self.temp.name) / "provenance.json"
        # A normal tag-triggered release loads helpers from its exact tag commit.
        tag_sha = self.git("rev-parse", "HEAD")
        result = subprocess.run(["python3", str(SCRIPT), "v0.97.0", "none", tag_sha], cwd=self.repo,
                                env=dict(os.environ, RELEASE_PROVENANCE_PATH=str(output)), capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertTrue(output.is_file())

    def test_invalid_input_unknown_patch_and_wrong_checkout_rejected(self):
        for tag, repair, sha in (("master", "none", None), ("v0.96.0;touch nope", "none", None),
                                 ("v0.96.0", "arbitrary", None), ("v0.97.0", "windows-listener", None),
                                 ("v0.96.0", "windows-listener", "master"),
                                 ("v0.96.0", "windows-listener", self.tag_sha)):
            with self.subTest(tag=tag, repair=repair, sha=sha):
                self.assertNotEqual(self.run_repair(tag, repair, sha).returncode, 0)
                self.assertEqual(self.git("diff", "--name-only"), "")

    def test_dirty_source_rejected_without_repair(self):
        source = self.repo / SOURCE
        content = source.read_bytes() + b"\n// unrelated change\n"
        source.write_bytes(content)
        self.assertNotEqual(self.run_repair().returncode, 0)
        self.assertEqual(source.read_bytes(), content)

    def test_windows_crlf_checkout(self):
        source = self.repo / SOURCE
        self.assertIn(b"\r\n", source.read_bytes())
        result = self.run_repair()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_public_state_refuses_public_newer_and_api_failure(self):
        directory = Path(self.temp.name) / "commands"
        directory.mkdir()
        gh = directory / "gh"
        gh.write_text('#!/bin/sh\n[ "${GH_FAIL:-0}" = 0 ] || exit 1\nif [ "$2" = view ]; then printf "%s" "$GH_DRAFT"; else printf "%s" "$GH_RELEASES"; fi\n')
        gh.chmod(0o755)
        for draft, releases, fail, succeeds in (("true", [], "0", True), ("false", [], "0", False),
                 ("true", [{"tagName": "v0.97.0"}], "0", False),
                 ("true", [{"tagName": "v0.95.0"}, {"tagName": "v0.96.0"}], "0", True),
                 ("true", [], "1", False)):
            with self.subTest(draft=draft, releases=releases, fail=fail):
                env = dict(os.environ, PATH=f"{directory}:{os.environ['PATH']}", GH_DRAFT=draft,
                           GH_RELEASES=json.dumps(releases), GH_FAIL=fail)
                result = subprocess.run(["python3", str(SCRIPT), "--check-public-state", "v0.96.0"],
                                        cwd=self.repo, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, succeeds, result.stderr)

    def test_windows_signing_handles_skipped_recovery_ancestor(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        signing = workflow.split("  publish-windows:\n", 1)[1].split("  build-freebsd:\n", 1)[0]
        condition = next(line.strip()[4:] for line in signing.splitlines()
                         if line.startswith("    if: "))
        self.assertEqual(condition, "always() && needs.create-release.result == 'success' && needs.build-windows.result == 'success'")
        self.assertIn("needs: [create-release, build-windows]", signing)
        # Exercise the checked-in predicate independently of ancestor status.
        # A skipped optional preflight must not skip successful signing inputs,
        # but failed/cancelled/skipped direct dependencies must stay blocked.
        for ancestor in ("success", "skipped"):
            for create in ("success", "failure", "cancelled", "skipped"):
                for build in ("success", "failure", "cancelled", "skipped"):
                    with self.subTest(ancestor=ancestor, create=create, build=build):
                        expression = condition.replace("always()", "True")
                        expression = expression.replace("needs.create-release.result", repr(create))
                        expression = expression.replace("needs.build-windows.result", repr(build))
                        expression = expression.replace("&&", "and")
                        self.assertEqual(eval(expression, {"__builtins__": {}}),
                                         create == "success" and build == "success")

    def test_workflow_identity_and_publication_gates(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertNotIn("GITHUB_REF_NAME", workflow)
        self.assertEqual(workflow.count("ref: ${{ needs.create-release.outputs.tag_sha }}"), 6)
        self.assertEqual(workflow.count("name: Apply reviewed release source recovery"), 6)
        self.assertIn("Require successful exact-commit CI for unpatched source", workflow)
        self.assertIn("Check and test exact repaired companion source", workflow)
        self.assertIn("Release tag no longer matches the triggering commit", workflow)
        self.assertEqual(workflow.count("needs: [create-release, recovery-preflight]"), 3)
        self.assertEqual(workflow.count("Verify release build preserved Cargo.lock"), 2)
        self.assertIn("cancel-in-progress: false", workflow)
        self.assertEqual(workflow.count('--check-public-state "$RELEASE_TAG"'), 2)
        self.assertIn("sha256sum release-provenance.json >> SHA256SUMS", workflow)
        self.assertIn('SHA256SUMS release-provenance.json --clobber', workflow)
        self.assertIn("Get-AuthenticodeSignature", workflow)
        windows = workflow.split("  build-windows:\n", 1)[1].split("  publish-windows:\n", 1)[0]
        preflight = workflow.split("  recovery-preflight:\n", 1)[1].split("  build-linux-macos:\n", 1)[0]
        self.assertEqual(workflow.count('JUSTRUST_DISABLE: "1"'), 2)
        self.assertIn('JUSTRUST_DISABLE: "1"', preflight)
        self.assertIn('JUSTRUST_DISABLE: "1"', windows)
        self.assertIn('& justrust @cargoArgs', windows)
        self.assertIn('@("build", "--local", "--locked", "--release"', windows)
        companion = (ROOT / ".github/workflows/windows-companion.yml").read_text()
        self.assertIn('JUSTRUST_DISABLE: "1"', companion)
        self.assertIn("justrust check --local --locked -p jcode-companion", companion)
        self.assertIn("Validate complete platform asset set", workflow)
        self.assertIn("Keeping the release as a draft until every platform succeeds", workflow)
        self.assertIn("if: github.event_name != 'workflow_dispatch'", workflow)


if __name__ == "__main__":
    unittest.main()
