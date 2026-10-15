#!/usr/bin/env python3
"""Authorize normal releases using only successful exact-commit push CI.

Requires gh on PATH and an inherited GH_TOKEN with Actions read permission.
Exit 0: authorized, 1: rejected/API error, 2: wait expired or invalid CLI input.
This gate deliberately does not authorize historical-tag recovery releases.
"""
import argparse
import json
import math
import re
import subprocess
import sys
import time

MANDATORY_JOBS = (
    "Quality Guardrails",
    "Build & Test (ubuntu-latest)",
    "Build & Test (macos-latest)",
    "Build & Test (windows-latest)",
    "Windows ARM64 Release Configuration",
)
PENDING = {"queued", "in_progress", "waiting", "pending", "requested"}


class Rejected(RuntimeError):
    """CI or its API response cannot authorize a release."""


class WaitExpired(RuntimeError):
    """Required CI evidence was not available within the wait budget."""


def validate_inputs(repository, sha, timeout, poll_interval):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*", repository):
        raise ValueError("repository must be OWNER/REPO")
    if not re.fullmatch(r"[0-9a-fA-F]{40}", sha):
        raise ValueError("SHA must be a full 40-character hexadecimal commit SHA")
    for name, value in (("timeout", timeout), ("poll interval", poll_interval)):
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"{name} must be finite and nonnegative")


def gh_api(endpoint, paginated=False, request_timeout=60):
    command = ["gh", "api", endpoint]
    if paginated:
        command += ["--paginate", "--slurp"]
    try:
        result = subprocess.run(command, check=True, capture_output=True,
                                text=True, timeout=request_timeout)
        return json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as exc:
        # Do not echo stderr, which may contain authentication information.
        raise Rejected(f"GitHub API error ({type(exc).__name__})") from exc


def objects(api, endpoint, key):
    pages = api(endpoint, paginated=True)
    if not isinstance(pages, list) or not pages:
        raise Rejected("Malformed paginated API response")
    result = []
    for page in pages:
        if not isinstance(page, dict) or not isinstance(page.get(key), list):
            raise Rejected(f"Malformed API {key} response")
        if not all(isinstance(item, dict) for item in page[key]):
            raise Rejected(f"Malformed API {key} item")
        result.extend(page[key])
    return result


def run_identity(run, sha):
    if (run.get("head_sha", "").lower() != sha or run.get("event") != "push"
            or run.get("head_branch") not in {"main", "master"}
            or run.get("path") not in {".github/workflows/ci.yml",
                                       ".github/workflows/ci.yml@main",
                                       ".github/workflows/ci.yml@master"}):
        raise Rejected("CI run identity does not match exact SHA/main push ci.yml")
    for field in ("id", "run_number", "run_attempt"):
        if type(run.get(field)) is not int or run[field] < 1:
            raise Rejected(f"Invalid CI run {field}")


def status_reason(item, label):
    status, conclusion = item.get("status"), item.get("conclusion")
    if conclusion not in (None, "success"):
        raise Rejected(f"{label} has non-success conclusion: {conclusion}")
    if status == "completed":
        if conclusion != "success":
            raise Rejected(f"{label} completed without success")
        return None
    if status in PENDING and conclusion is None:
        return f"{label} is {status}"
    raise Rejected(f"{label} has invalid status/conclusion")


def check_once(repository, sha, api):
    base = f"repos/{repository}/actions"
    runs = objects(api, f"{base}/workflows/ci.yml/runs?head_sha={sha}&event=push&per_page=100", "workflow_runs")
    # Reject different-SHA green evidence rather than trusting API query filtering.
    candidates = [run for run in runs if run.get("head_sha", "").lower() == sha
                  and run.get("event") == "push" and run.get("head_branch") in {"main", "master"}]
    if not candidates:
        return "missing exact-SHA main push CI run"
    for run in candidates:
        run_identity(run, sha)
    latest = max(candidates, key=lambda run: (run["run_number"], run["id"], run["run_attempt"]))
    run = api(f"{base}/runs/{latest['id']}")
    if not isinstance(run, dict):
        raise Rejected("Malformed CI run response")
    run_identity(run, sha)
    if run["id"] != latest["id"] or run["run_attempt"] < latest["run_attempt"]:
        raise Rejected("CI run detail does not match latest run/attempt")
    reason = status_reason(run, f"CI run {run['id']} attempt {run['run_attempt']}")
    jobs = objects(api, f"{base}/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100", "jobs")
    pending = []
    for name in MANDATORY_JOBS:
        matches = [job for job in jobs if job.get("name") == name]
        if not matches:
            if run["status"] == "completed":
                raise Rejected(f"Completed CI run missing mandatory job: {name}")
            pending.append(f"missing mandatory job: {name}")
            continue
        if len(matches) != 1:
            raise Rejected(f"Ambiguous mandatory job: {name}")
        job = matches[0]
        # The attempt-specific endpoint is authoritative. GitHub's documented
        # job schema does not require run_attempt, but reject contradictions.
        if "run_attempt" in job and job["run_attempt"] != run["run_attempt"]:
            raise Rejected(f"Wrong attempt for mandatory job: {name}")
        if (("run_id" in job and job["run_id"] != run["id"])
                or ("head_sha" in job and job["head_sha"] != sha)):
            raise Rejected(f"Wrong run/SHA for mandatory job: {name}")
        job_reason = status_reason(job, name)
        if job_reason:
            pending.append(job_reason)
    if reason or pending:
        return "; ".join(([reason] if reason else []) + pending)
    # A concurrent rerun must not allow stale attempt success to authorize.
    current = api(f"{base}/runs/{run['id']}")
    if not isinstance(current, dict):
        raise Rejected("Malformed CI run confirmation")
    run_identity(current, sha)
    if current["id"] != run["id"]:
        raise Rejected("CI run changed identity")
    confirmation = status_reason(current, "CI run confirmation")
    if current["run_attempt"] != run["run_attempt"]:
        return "CI run attempt changed during verification"
    return confirmation


def authorize(repository, sha, timeout=7200, poll_interval=30, api=gh_api,
              clock=time.monotonic, sleep=time.sleep, emit=print):
    validate_inputs(repository, sha, timeout, poll_interval)
    sha = sha.lower()
    deadline = clock() + timeout
    while True:
        reason = check_once(repository, sha, api)
        if reason is None:
            emit(f"AUTHORIZED: exact-SHA main push CI and mandatory jobs succeeded for {sha}")
            return
        remaining = deadline - clock()
        if remaining <= 0:
            raise WaitExpired(f"WAIT EXPIRED: {reason}")
        emit(f"WAITING: {reason}")
        sleep(min(poll_interval, remaining))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag_sha", nargs="?", help="full tag commit SHA")
    parser.add_argument("--repository", "--repo", dest="repository", required=True)
    parser.add_argument("--sha", help="full tag commit SHA (alternative to positional SHA)")
    parser.add_argument("--timeout", type=float, default=7200)
    parser.add_argument("--poll-interval", type=float, default=30)
    args = parser.parse_args(argv)
    if bool(args.sha) == bool(args.tag_sha):
        parser.error("provide exactly one positional SHA or --sha")
    try:
        authorize(args.repository, args.sha or args.tag_sha, args.timeout, args.poll_interval)
    except (ValueError, WaitExpired) as exc:
        print(str(exc), file=sys.stderr)
        return 2
    except Rejected as exc:
        print(f"REJECTED: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
