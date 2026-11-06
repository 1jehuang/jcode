#!/usr/bin/env python3
"""Bounded preview recovery contracts, with no network writes."""
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import reconcile_preview_release as recovery

SHA = "a" * 40
TAG = "v1.0.0-preview.12"
ROOT = Path(__file__).resolve().parents[1]


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.runs = []
        self.refs = []
        self.releases = []
        self.heads = [SHA, SHA]
        self.calls = []
        self.messages = []
        self.ci = patch.object(recovery, "check_once", return_value=None).start()
        self.addCleanup(patch.stopall)

    def api(self, endpoint, paginated=False, **kwargs):
        self.calls.append((endpoint, paginated, kwargs))
        if kwargs:
            self.assertEqual(kwargs, {"method": "POST", "fields": {"ref": "master"}})
            self.assertTrue(endpoint.endswith("/preview.yml/dispatches"))
            return None
        if endpoint.endswith("/git/ref/heads/master"):
            return {"object": {"sha": self.heads.pop(0)}}
        if "/git/matching-refs/" in endpoint:
            self.assertTrue(paginated)
            return [[], self.refs]
        if "/preview.yml/runs?" in endpoint:
            self.assertTrue(paginated)
            return [{"workflow_runs": []}, {"workflow_runs": self.runs}]
        if "/releases?" in endpoint:
            self.assertTrue(paginated)
            return [[], self.releases]
        self.fail(endpoint)

    def run_recovery(self):
        return recovery.reconcile(api=self.api, emit=self.messages.append)

    def run_record(self, status="completed", sha=SHA, branch="master"):
        return {"head_sha": sha, "head_branch": branch, "status": status,
                "conclusion": "failure" if status == "completed" else None}

    def tag(self, sha=SHA):
        self.refs.append({"ref": f"refs/tags/{TAG}", "object": {"sha": sha}})

    def test_failed_preview_recovers_after_ci_rerun(self):
        self.runs = [self.run_record()]
        self.assertTrue(self.run_recovery())
        self.ci.assert_called_once_with(recovery.REPOSITORY, SHA, self.api, preview=True)
        self.assertIn("2/3", self.messages[-1])
        self.assertTrue(self.calls[-1][2])

    def test_missing_push_workflow_can_be_dispatched(self):
        self.assertTrue(self.run_recovery())
        self.assertIn("1/3", self.messages[-1])

    def test_pending_and_failed_source_ci_never_dispatch(self):
        for outcome in ("missing exact-SHA main push CI run", recovery.Rejected("compile failed")):
            with self.subTest(outcome=outcome):
                self.heads = [SHA, SHA]
                self.calls.clear()
                if isinstance(outcome, Exception):
                    self.ci.side_effect = outcome
                else:
                    self.ci.return_value = outcome
                self.assertFalse(self.run_recovery())
                self.assertFalse(any(call[2] for call in self.calls))

    def test_active_preview_is_never_cancelled(self):
        for status in recovery.ACTIVE:
            with self.subTest(status=status):
                self.heads = [SHA, SHA]
                self.runs = [self.run_record(status)]
                self.assertFalse(self.run_recovery())
        self.ci.assert_not_called()

    def test_automatic_retries_are_bounded(self):
        self.runs = [self.run_record() for _ in range(recovery.MAX_ATTEMPTS)]
        self.assertFalse(self.run_recovery())
        self.ci.assert_not_called()
        self.assertIn("limit", self.messages[-1])

    def test_other_shas_and_branches_do_not_consume_budget(self):
        self.runs = [self.run_record(sha="b" * 40), self.run_record(branch="feature")] * 4
        self.assertTrue(self.run_recovery())
        self.assertIn("1/3", self.messages[-1])

    def test_published_current_sha_is_not_republished(self):
        self.tag()
        self.releases = [{"draft": False, "prerelease": True, "tag_name": TAG,
                          "target_commitish": "master"}]
        self.assertFalse(self.run_recovery())
        self.ci.assert_not_called()

    def test_draft_and_tag_only_interruptions_get_fresh_attempt(self):
        self.tag()
        self.releases = [{"draft": True, "prerelease": True, "tag_name": TAG}]
        self.assertTrue(self.run_recovery())

    def test_other_commit_release_is_not_current_even_with_master_target(self):
        self.tag(sha="b" * 40)
        self.releases = [{"draft": False, "prerelease": True, "tag_name": TAG,
                          "target_commitish": "master"}]
        self.assertTrue(self.run_recovery())

    def test_stable_v1_ends_recovery(self):
        self.refs = [{"ref": "refs/tags/v1.0.0", "object": {"sha": SHA}}]
        self.assertFalse(self.run_recovery())
        self.ci.assert_not_called()

    def test_master_advance_prevents_dispatch(self):
        self.heads = [SHA, "b" * 40]
        self.assertFalse(self.run_recovery())
        self.assertIn("advanced", self.messages[-1])

    def test_api_error_does_not_dispatch(self):
        self.ci.side_effect = recovery.Rejected("GitHub API error")
        self.assertFalse(self.run_recovery())
        self.assertFalse(any(call[2] for call in self.calls))

    def test_fork_rejected(self):
        with self.assertRaises(ValueError):
            recovery.reconcile(repository="fork/jcode", api=self.api)
        self.assertEqual(self.calls, [])

    def test_workflow_only_executes_trusted_master_with_no_publish_permission(self):
        workflow = (ROOT / ".github/workflows/preview-recovery.yml").read_text()
        for required in ("workflows: [CI]", "types: [completed]", "branches: [master]",
                         "schedule:", "cancel-in-progress: false", "contents: read",
                         "actions: write", "ref: master", "persist-credentials: false",
                         "github.event.workflow_run.event == 'push'",
                         "github.event.workflow_run.head_repository.full_name == github.repository",
                         "vars.JCODE_V1_PREVIEW_ENABLED != 'false'"):
            self.assertIn(required, workflow)
        self.assertNotIn("contents: write", workflow)
        self.assertNotIn("download-artifact", workflow)
        self.assertNotIn("head_sha }}", workflow)
        # API list consistency must not let a retry cancel an unseen active build.
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'push' }}",
                      (ROOT / ".github/workflows/preview.yml").read_text())
        self.assertIn("python3 scripts/test_reconcile_preview_release.py",
                      (ROOT / ".github/workflows/ci.yml").read_text())


if __name__ == "__main__":
    unittest.main()
