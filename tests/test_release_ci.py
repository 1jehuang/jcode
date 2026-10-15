#!/usr/bin/env python3
import copy
import importlib.util
import pathlib
import subprocess
import unittest
from unittest.mock import Mock, patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("release_ci", ROOT / "scripts/check_release_ci.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
SHA = "a" * 40
REPO = "owner/repo"


def run(**changes):
    result = dict(id=100, run_number=10, run_attempt=1, head_sha=SHA,
                  event="push", head_branch="master", path=".github/workflows/ci.yml",
                  status="completed", conclusion="success")
    result.update(changes)
    return result


def jobs(attempt=1):
    return [dict(name=name, status="completed", conclusion="success", run_attempt=attempt)
            for name in gate.MANDATORY_JOBS]


class FakeAPI:
    def __init__(self, runs=None, detail=None, job_list=None, confirmation=None):
        self.runs = [run()] if runs is None else runs
        self.detail = run() if detail is None else detail
        self.jobs = jobs() if job_list is None else job_list
        self.confirmation = confirmation
        self.calls = []
        self.details = 0

    def __call__(self, endpoint, paginated=False):
        self.calls.append((endpoint, paginated))
        if "/workflows/ci.yml/runs?" in endpoint:
            return [{"workflow_runs": copy.deepcopy(self.runs)}]
        if "/jobs?" in endpoint:
            return [{"jobs": copy.deepcopy(self.jobs)}]
        self.details += 1
        return copy.deepcopy(self.confirmation if self.details > 1 and self.confirmation else self.detail)


class ReleaseCITests(unittest.TestCase):
    def authorize(self, api, **kwargs):
        return gate.authorize(REPO, SHA, timeout=0, api=api, emit=lambda _: None, **kwargs)

    def test_success_queries_only_exact_sha_push_and_attempt(self):
        api = FakeAPI()
        self.authorize(api)
        self.assertIn(f"head_sha={SHA}&event=push", api.calls[0][0])
        self.assertIn("/attempts/1/jobs?", api.calls[2][0])
        self.assertTrue(api.calls[2][1])

    def test_different_sha_branch_or_event_never_authorizes(self):
        for changes in (dict(head_sha="b" * 40), dict(head_branch="feature"), dict(event="pull_request")):
            with self.subTest(changes=changes), self.assertRaises(gate.WaitExpired):
                self.authorize(FakeAPI(runs=[run(**changes)]))

    def test_missing_run_wait_expires(self):
        with self.assertRaisesRegex(gate.WaitExpired, "missing exact-SHA"):
            self.authorize(FakeAPI(runs=[]))

    def test_failed_run_immediately_rejected_without_job_queries(self):
        for conclusion in ("failure", "cancelled", "timed_out", "action_required", "skipped", "neutral", "stale", "error"):
            api = FakeAPI(detail=run(conclusion=conclusion))
            with self.subTest(conclusion=conclusion), self.assertRaises(gate.Rejected):
                self.authorize(api)
            self.assertEqual(len(api.calls), 2)

    def test_pending_run_wait_expires_even_with_green_jobs(self):
        with self.assertRaisesRegex(gate.WaitExpired, "in_progress"):
            self.authorize(FakeAPI(detail=run(status="in_progress", conclusion=None)))

    def test_each_mandatory_job_missing_failed_pending(self):
        for index, name in enumerate(gate.MANDATORY_JOBS):
            with self.subTest(name=name, state="missing"), self.assertRaisesRegex(gate.Rejected, "missing mandatory job"):
                self.authorize(FakeAPI(job_list=jobs()[:index] + jobs()[index + 1:]))
            for conclusion in ("failure", "cancelled", "skipped", "neutral", "timed_out"):
                items = jobs()
                items[index]["conclusion"] = conclusion
                with self.subTest(name=name, state=conclusion), self.assertRaises(gate.Rejected):
                    self.authorize(FakeAPI(job_list=items))
            items = jobs()
            items[index].update(status="queued", conclusion=None)
            with self.subTest(name=name, state="pending"), self.assertRaises(gate.WaitExpired):
                self.authorize(FakeAPI(job_list=items))

    def test_pending_run_missing_jobs_still_waits(self):
        with self.assertRaisesRegex(gate.WaitExpired, "missing mandatory job"):
            self.authorize(FakeAPI(detail=run(status="in_progress", conclusion=None), job_list=[]))

    def test_newest_run_never_falls_back_to_older_pass(self):
        api = FakeAPI(runs=[run(), run(id=101, run_number=11, conclusion="failure")],
                      detail=run(id=101, run_number=11, conclusion="failure"))
        with self.assertRaises(gate.Rejected):
            self.authorize(api)
        self.assertTrue(api.calls[1][0].endswith("/runs/101"))

    def test_newest_attempt_not_older_pass(self):
        api = FakeAPI(runs=[run(), run(run_attempt=2)], detail=run(run_attempt=2), job_list=jobs(2))
        self.authorize(api)
        self.assertIn("/attempts/2/jobs?", api.calls[2][0])
        api = FakeAPI(detail=run(run_attempt=2, conclusion="failure"))
        with self.assertRaises(gate.Rejected):
            self.authorize(api)
        with self.assertRaisesRegex(gate.Rejected, "Wrong attempt"):
            self.authorize(FakeAPI(detail=run(run_attempt=2), job_list=jobs(1)))

    def test_rerun_race_cannot_authorize_old_attempt(self):
        with self.assertRaisesRegex(gate.WaitExpired, "attempt changed"):
            self.authorize(FakeAPI(confirmation=run(run_attempt=2)))

    def test_detail_identity_mismatch_fails(self):
        for changes in (dict(head_sha="b" * 40), dict(id=101), dict(path="other.yml"), dict(event="pull_request")):
            with self.subTest(changes=changes), self.assertRaises(gate.Rejected):
                self.authorize(FakeAPI(detail=run(**changes)))

    def test_documented_jobs_without_attempt_field(self):
        items = jobs()
        for job in items:
            del job["run_attempt"]
            job.update(run_id=100, head_sha=SHA)
        self.authorize(FakeAPI(job_list=items, detail=run(path=".github/workflows/ci.yml@master")))
        for changes in (dict(run_id=101), dict(head_sha="b" * 40)):
            invalid = copy.deepcopy(items)
            invalid[0].update(changes)
            with self.subTest(changes=changes), self.assertRaises(gate.Rejected):
                self.authorize(FakeAPI(job_list=invalid))

    def test_ambiguous_jobs_fail(self):
        with self.assertRaisesRegex(gate.Rejected, "Ambiguous"):
            self.authorize(FakeAPI(job_list=jobs() + [jobs()[0]]))

    def test_malformed_api_fails(self):
        for response in (None, {}, [], [{"workflow_runs": None}], [{"workflow_runs": [None]}]):
            with self.subTest(response=response), self.assertRaises(gate.Rejected):
                self.authorize(Mock(return_value=response))

    def test_api_errors_no_retry_and_no_secret_output(self):
        errors = (OSError("secret"), subprocess.CalledProcessError(1, "gh", stderr="secret"),
                  subprocess.TimeoutExpired("gh", 60))
        for error in errors:
            with self.subTest(error=error), patch.object(gate.subprocess, "run", side_effect=error) as process:
                with self.assertRaises(gate.Rejected) as caught:
                    gate.gh_api("repos/owner/repo/actions/runs")
                self.assertNotIn("secret", str(caught.exception))
                process.assert_called_once()
        with patch.object(gate.subprocess, "run", return_value=Mock(stdout="not json")):
            with self.assertRaises(gate.Rejected):
                gate.gh_api("repos/owner/repo/actions/runs")

    def test_process_fixed_gh_inherits_environment(self):
        with patch.object(gate.subprocess, "run", return_value=Mock(stdout='[{"jobs": []}]')) as process:
            gate.gh_api("repos/owner/repo/actions/runs/1/jobs", paginated=True)
        args, kwargs = process.call_args
        self.assertEqual(args[0], ["gh", "api", "repos/owner/repo/actions/runs/1/jobs", "--paginate", "--slurp"])
        self.assertNotIn("env", kwargs)
        self.assertNotIn("shell", kwargs)
        self.assertEqual(kwargs["timeout"], 60)

    def test_bounded_wait_clamps_last_sleep(self):
        now = [0]
        sleeps = []
        def sleep(seconds):
            sleeps.append(seconds)
            now[0] += seconds
        with self.assertRaises(gate.WaitExpired):
            gate.authorize(REPO, SHA, timeout=5, poll_interval=3, api=FakeAPI(runs=[]),
                           clock=lambda: now[0], sleep=sleep, emit=lambda _: None)
        self.assertEqual(sleeps, [3, 2])

    def test_pending_can_become_success(self):
        api = FakeAPI(runs=[])
        def sleep(_):
            api.runs = [run()]
        gate.authorize(REPO, SHA, timeout=1, poll_interval=0, api=api,
                       sleep=sleep, clock=lambda: 0, emit=lambda _: None)

    def test_inputs_and_cli_exit_statuses(self):
        for repo in ("../repo", "owner/repo?token=x", "owner/repo/more", "-owner/repo"):
            with self.subTest(repo=repo), self.assertRaises(ValueError):
                gate.validate_inputs(repo, SHA, 0, 0)
        for sha in ("a" * 39, "g" * 40, "main", SHA + "\n"):
            with self.subTest(sha=sha), self.assertRaises(ValueError):
                gate.validate_inputs(REPO, sha, 0, 0)
        for value in (-1, float("inf"), float("nan")):
            for timeout, interval in ((value, 0), (0, value)):
                with self.assertRaises(ValueError):
                    gate.validate_inputs(REPO, SHA, timeout, interval)
        gate.validate_inputs(REPO, SHA.upper(), 0, 0)
        argv = ["--repository", REPO, "--sha", SHA]
        for error, code in ((gate.Rejected("failure"), 1), (gate.WaitExpired("pending"), 2), (ValueError("input"), 2)):
            with patch.object(gate, "authorize", side_effect=error), patch("sys.stderr"):
                self.assertEqual(gate.main(argv), code)
        with patch.object(gate, "authorize"):
            self.assertEqual(gate.main(argv), 0)

    def test_wired_release_cli(self):
        with patch.object(gate, "authorize") as authorize:
            self.assertEqual(gate.main([SHA, "--repo", REPO, "--timeout", "10800"]), 0)
            authorize.assert_called_once_with(REPO, SHA, 10800, 30)
        for argv in (["--repo", REPO], [SHA, "--sha", SHA, "--repo", REPO]):
            with patch("sys.stderr"), self.assertRaises(SystemExit) as caught:
                gate.main(argv)
            self.assertEqual(caught.exception.code, 2)

    def test_job_names_match_actual_workflow(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("name: Quality Guardrails", workflow)
        self.assertIn("name: Build & Test (${{ matrix.os }})", workflow)
        self.assertIn("os: [ubuntu-latest, macos-latest]", workflow)
        self.assertIn("name: Build & Test (windows-latest)", workflow)
        self.assertIn("name: Windows ARM64 Release Configuration", workflow)


if __name__ == "__main__":
    unittest.main()
