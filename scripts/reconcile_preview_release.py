#!/usr/bin/env python3
"""Reconcile unpublished master previews without bypassing exact-source CI.

Runs after CI completion and periodically for transient build/publish failures.
Only dispatches the existing, fail-closed preview workflow. Never reruns failed
source CI, publishes assets, or consumes code from a workflow_run artifact.
"""
import re
import subprocess

from check_release_ci import Rejected, check_once, gh_api, objects
from preview_release import REPOSITORY

MAX_ATTEMPTS = 3
ACTIVE = {"queued", "in_progress", "waiting", "pending", "requested"}
PREVIEW_TAG = re.compile(r"v1\.0\.0-preview\.[1-9][0-9]*")


def reconcile(repository=REPOSITORY, api=None, emit=print):
    api = api or api_with_dispatch
    if repository != REPOSITORY:
        raise ValueError("Preview recovery is restricted to the canonical repository")
    sha = api(f"repos/{repository}/git/ref/heads/master")["object"]["sha"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid master SHA")
    # The stable tag ends automation, even if an old preview job failed.
    pages = api(f"repos/{repository}/git/matching-refs/tags/v1.0.0?per_page=100", paginated=True)
    if not isinstance(pages, list) or not all(isinstance(page, list) for page in pages):
        raise ValueError("Malformed preview tag response")
    refs = [ref for page in pages for ref in page]
    if any(ref["ref"] == "refs/tags/v1.0.0" for ref in refs):
        emit("SKIP: v1.0.0 stable exists, preview window has ended")
        return False
    runs = objects(api, f"repos/{repository}/actions/workflows/preview.yml/runs?head_sha={sha}&per_page=100",
                   "workflow_runs")
    runs = [run for run in runs if run.get("head_sha") == sha
            and run.get("head_branch") == "master"]
    if any(run.get("status") in ACTIVE for run in runs):
        emit("SKIP: current master already has an active preview")
        return False
    # Match published release tags to their actual commit, not target_commitish
    # (GitHub may return the mutable branch name there). Ignore orphaned drafts.
    tags = {ref["ref"].removeprefix("refs/tags/") for ref in refs
            if ref.get("object", {}).get("sha") == sha}
    if tags:
        pages = api(f"repos/{repository}/releases?per_page=100", paginated=True)
        if not isinstance(pages, list) or not all(isinstance(page, list) for page in pages):
            raise ValueError("Malformed releases response")
        if any(not release["draft"] and release["prerelease"]
               and PREVIEW_TAG.fullmatch(release["tag_name"])
               and release["tag_name"] in tags for page in pages for release in page):
            emit("SKIP: current master already has a published preview")
            return False
    if len(runs) >= MAX_ATTEMPTS:
        emit(f"SKIP: automatic preview attempt limit ({MAX_ATTEMPTS}) reached for {sha}")
        return False
    try:
        reason = check_once(repository, sha, api, preview=True)
    except Rejected as exc:
        emit(f"SKIP: source CI is not authorized: {exc}")
        return False
    if reason:
        emit(f"SKIP: source CI is not ready: {reason}")
        return False
    # Recheck immediately before dispatch. The dispatched pipeline repeats its
    # quiet period, exact-source authorization, and final-head publication gate.
    if api(f"repos/{repository}/git/ref/heads/master")["object"]["sha"] != sha:
        emit("SKIP: master advanced during reconciliation")
        return False
    api(f"repos/{repository}/actions/workflows/preview.yml/dispatches",
        method="POST", fields={"ref": "master"})
    emit(f"DISPATCHED: preview attempt {len(runs) + 1}/{MAX_ATTEMPTS} for {sha}")
    return True


def api_with_dispatch(endpoint, paginated=False, method=None, fields=None):
    if method is None:
        return gh_api(endpoint, paginated=paginated)
    command = ["gh", "api", endpoint, "--method", method]
    for key, value in fields.items():
        command.extend(["-f", f"{key}={value}"])
    subprocess.run(command, check=True, capture_output=True, text=True, timeout=60)


if __name__ == "__main__":
    reconcile(api=api_with_dispatch)
