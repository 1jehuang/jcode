#!/usr/bin/env python3
"""Fail-closed publication for the opt-in v1 preview channel only.

No stable metadata, registry writes, workflow dispatches, or external tokens.
GITHUB_TOKEN-created tags/releases deliberately cannot trigger other workflows.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

from check_release_ci import authorize

REPOSITORY = "1jehuang/jcode"
ASSETS = tuple(f"jcode-{platform}-{arch}.tar.gz" for platform in ("linux", "macos", "windows")
               for arch in ("x86_64", "aarch64"))


def gh(*args):
    result = subprocess.run(["gh", *args], check=True, capture_output=True, text=True)
    return result.stdout


def api(endpoint):
    return json.loads(gh("api", endpoint))


def validate(repository, sha, tag):
    if repository != REPOSITORY:
        raise ValueError("Preview publication is restricted to the canonical repository")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Expected full lowercase commit SHA")
    if not re.fullmatch(r"v1\.0\.0-preview\.[1-9][0-9]*", tag):
        raise ValueError("Expected v1.0.0-preview.N")


def require_preview_window(repository):
    pages = json.loads(gh("api", "--paginate", "--slurp",
                         f"repos/{repository}/git/matching-refs/tags/v1.0.0"))
    if any(ref.get("ref") == "refs/tags/v1.0.0" for page in pages for ref in page):
        raise ValueError("v1.0.0 stable tag exists, preview publishing has ended")


def require_head(repository, sha):
    if api(f"repos/{repository}/git/ref/heads/master")["object"]["sha"] != sha:
        raise ValueError("master advanced, refusing stale preview")


def inventory(directory):
    directory = Path(directory)
    files = {p.name: p for p in directory.iterdir() if p.is_file()}
    if set(files) != set(ASSETS):
        raise ValueError(f"Expected exact six-platform preview asset set, found {sorted(files)}")
    if any(p.stat().st_size == 0 for p in files.values()):
        raise ValueError("Empty preview asset")
    return files


def publish(repository, sha, tag, directory):
    validate(repository, sha, tag)
    files = inventory(directory)
    # Recheck CI after the builds as well as before them. No stale/rerun evidence.
    authorize(repository, sha, timeout=0)
    require_preview_window(repository)
    require_head(repository, sha)
    checksum = Path(directory) / "SHA256SUMS"
    checksum.write_text("".join(f"{hashlib.sha256(files[name].read_bytes()).hexdigest()}  {name}\n"
                                for name in sorted(files)))
    # Never overwrite or reuse a tag/draft from an interrupted attempt. Every
    # workflow run has its own monotonically increasing preview number.
    tags = json.loads(gh("api", "--paginate", "--slurp",
                         f"repos/{repository}/git/matching-refs/tags/{tag}"))
    if any(ref.get("ref") == f"refs/tags/{tag}" for page in tags for ref in page):
        raise ValueError("Preview tag already exists, start a new workflow run")
    gh("api", f"repos/{repository}/git/refs", "-X", "POST",
       "-f", f"ref=refs/tags/{tag}", "-f", f"sha={sha}")
    notes = (f"Opt-in v1 preview built from master commit {sha}.\n\n"
             "Experimental binaries, not a stable release. Windows preview binaries are unsigned. "
             "Use the stable channel for production. Preview numbers may have gaps.\n\n"
             "## First-time preview setup\n\n"
             "Existing stable versions do not support `--channel`. Download the archive for your "
             "OS and architecture from this release, verify it against `SHA256SUMS`, and extract "
             "the entire archive into its own directory (keep all Linux companion files together). "
             "Run the extracted binary explicitly, using its full path, rather than the old stable "
             "`jcode` on PATH. For Windows, use the extracted `.exe`.\n\n"
             "From that preview binary, run `jcode update --channel preview` (substitute its "
             "full path for `jcode`) to persist the opt-in channel and receive future previews. "
             "To roll back, run the preview binary with `update --channel stable`, or reinstall "
             "the current stable release. Stable users are never moved to previews automatically.\n\n"
             "This preview series ends when the `v1.0.0` stable tag is created.\n")
    gh("release", "create", tag, "--repo", repository, "--verify-tag", "--draft",
       "--prerelease", "--latest=false", "--title", tag, "--notes", notes,
       *[str(files[name]) for name in sorted(files)], str(checksum))
    release = json.loads(gh("release", "view", tag, "--repo", repository,
                            "--json", "isDraft,isPrerelease,assets"))
    expected = {**files, "SHA256SUMS": checksum}
    assets = release["assets"]
    if (not release["isDraft"] or not release["isPrerelease"]
            or len(assets) != len(expected)
            or {asset["name"] for asset in assets} != set(expected)
            or any(asset.get("state") != "uploaded" or asset.get("size") != expected[asset["name"]].stat().st_size
                   for asset in assets)):
        raise ValueError("Draft assets incomplete, refusing publication")
    if api(f"repos/{repository}/git/ref/tags/{tag}")["object"]["sha"] != sha:
        raise ValueError("Preview tag changed during staging")
    require_preview_window(repository)
    # This MUST remain the final check immediately before the visibility change.
    require_head(repository, sha)
    gh("release", "edit", tag, "--repo", repository, "--draft=false", "--prerelease", "--latest=false")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("check-head", "publish"))
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--sha", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--assets", default="dist")
    args = parser.parse_args()
    validate(args.repo, args.sha, args.tag)
    if args.action == "check-head":
        require_preview_window(args.repo)
        require_head(args.repo, args.sha)
    else:
        publish(args.repo, args.sha, args.tag, args.assets)


if __name__ == "__main__":
    main()
