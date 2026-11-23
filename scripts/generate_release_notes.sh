#!/usr/bin/env bash
set -euo pipefail

# Generate a human-readable release notes body for a jcode release (issue #435).
#
# Usage:
#   scripts/generate_release_notes.sh <tag> [previous-tag]
#
# Behavior:
#   1. If changelog/v<version>.json exists (schema in changelog/README.md),
#      render it into Highlights / Improvements / Fixes markdown sections.
#   2. Otherwise, group commit subjects between the previous tag and <tag>
#      by conventional-commit prefix (feat/fix/perf/docs/other).
#   Always appends the GitHub compare link at the bottom.

TAG="${1:?Usage: scripts/generate_release_notes.sh <tag> [previous-tag]}"
PREV_TAG="${2:-}"

cd "$(git rev-parse --show-toplevel)"

VERSION_NUM="${TAG#v}"
CHANGELOG_FILE="changelog/v${VERSION_NUM}.json"

# Determine the owner/repo slug for the compare link.
repo_slug() {
    if [[ -n "${GITHUB_REPOSITORY:-}" ]]; then
        echo "$GITHUB_REPOSITORY"
        return
    fi
    local url
    url="$(git remote get-url origin 2>/dev/null || true)"
    if [[ "$url" =~ github\.com[:/]+([^/]+/[^/ ]+) ]]; then
        echo "${BASH_REMATCH[1]%.git}"
        return
    fi
    echo "1jehuang/jcode"
}

REPO="$(repo_slug)"

# Prefer the latest *published* stable release on GitHub so unpublished drafts
# (such as aborted release attempts) are never used as the baseline and cannot
# hide bundled commits from the next release's notes or compare link. When the
# GitHub API is unavailable, fall back to changelog/index.json (skipping entries
# explicitly marked unpublished/draft), then local git tag history.
if [[ -z "$PREV_TAG" ]]; then
    PREV_TAG="$(python3 - "$VERSION_NUM" "$REPO" << 'PY'
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.request

def version(value):
    value = value.lstrip("v")
    if re.fullmatch(r"\d+\.\d+\.\d+", value):
        return tuple(map(int, value.split(".")))
    return None

def local_tag_exists(tag):
    return subprocess.run(
        ["git", "rev-parse", "-q", "--verify", f"refs/tags/{tag}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode == 0

def published_tags_from_github(repo, current):
    if os.environ.get("JCODE_SKIP_GITHUB_API") == "1":
        return []
    fixture = os.environ.get("JCODE_PUBLISHED_RELEASES_JSON")
    if fixture is not None:
        releases = json.loads(fixture)
        return [
            r.get("tag_name") or r.get("tagName") or ""
            for r in releases
            if not r.get("draft") and not r.get("prerelease")
        ]
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        tags = []
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "User-Agent": "jcode-release-notes/1.0",
        }
        for page in range(1, 6):
            url = f"https://api.github.com/repos/{repo}/releases?per_page=100&page={page}"
            req = urllib.request.Request(url, headers=headers)
            try:
                with urllib.request.urlopen(req, timeout=15) as resp:
                    batch = json.load(resp)
            except (urllib.error.URLError, TimeoutError, ValueError):
                return []
            if not batch:
                break
            found_older = False
            for rel in batch:
                if rel.get("draft") or rel.get("prerelease"):
                    continue
                tag_name = rel.get("tag_name") or ""
                ver = version(tag_name)
                if ver is not None:
                    tags.append(tag_name)
                    if ver < current:
                        found_older = True
            if found_older or len(batch) < 100:
                break
        return tags
    if shutil.which("gh"):
        try:
            out = subprocess.check_output(
                [
                    "gh", "release", "list",
                    "--repo", repo,
                    "--exclude-drafts",
                    "--exclude-pre-releases",
                    "--limit", "50",
                    "--json", "tagName",
                ],
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=10,
            )
            return [r.get("tagName") or "" for r in json.loads(out)]
        except (subprocess.SubprocessError, ValueError):
            return []
    return []

current = version(sys.argv[1])
repo = sys.argv[2]
if current:
    gh_candidates = [
        tag for tag in published_tags_from_github(repo, current)
        if version(tag) is not None and version(tag) < current and local_tag_exists(tag)
    ]
    if gh_candidates:
        print(max(gh_candidates, key=version))
        sys.exit(0)

    index = pathlib.Path("changelog/index.json")
    if index.exists():
        entries = json.loads(index.read_text())["entries"]
        candidates = [
            "v" + entry["version"].lstrip("v")
            for entry in entries
            if entry.get("published", True) is not False
            and entry.get("draft", False) is not True
            and version(entry["version"]) is not None
            and version(entry["version"]) < current
            and local_tag_exists("v" + entry["version"].lstrip("v"))
        ]
        if candidates:
            print(max(candidates, key=version))
PY
)"
    if [[ -z "$PREV_TAG" ]] && git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
        PREV_TAG="$(git describe --tags --abbrev=0 "$TAG^" 2>/dev/null || true)"
    fi
    if [[ -z "$PREV_TAG" ]]; then
        PREV_TAG="$(git tag -l 'v*' --sort=-v:refname \
            | grep -A1 -Fx "$TAG" | tail -n +2 | head -1 || true)"
    fi
fi

# Render changelog/v<version>.json into markdown sections.
render_changelog_json() {
    python3 - "$CHANGELOG_FILE" << 'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    entry = json.load(f)

out = []
title = entry.get("title")
if title:
    out.append(f"**{title}**")
    out.append("")

for key, heading in (
    ("highlights", "Highlights"),
    ("improvements", "Improvements"),
    ("fixes", "Fixes"),
):
    items = entry.get(key) or []
    if not items:
        continue
    out.append(f"### {heading}")
    out.append("")
    out.extend(f"- {item}" for item in items)
    out.append("")

print("\n".join(out).rstrip())
PY
}

# Fallback: group commit subjects since the previous tag by
# conventional-commit prefix.
render_commit_fallback() {
    local range
    local end_ref="$TAG"
    # Support pre-tag dry runs where the tag does not exist yet.
    if ! git rev-parse -q --verify "$TAG^{commit}" >/dev/null; then
        end_ref="HEAD"
    fi
    if [[ -n "$PREV_TAG" ]]; then
        range="$PREV_TAG..$end_ref"
    else
        range="$end_ref"
    fi

    python3 - "$range" << 'PY'
import re
import subprocess
import sys

subjects = subprocess.run(
    ["git", "log", "--no-merges", "--pretty=format:%s", sys.argv[1]],
    check=True,
    capture_output=True,
    text=True,
).stdout.splitlines()

groups = {"feat": [], "fix": [], "perf": [], "docs": [], "other": []}
pattern = re.compile(r"^(feat|fix|perf|docs)(\([^)]*\))?!?:\s*(.+)$", re.IGNORECASE)
for subject in subjects:
    subject = subject.strip()
    if not subject:
        continue
    match = pattern.match(subject)
    if match:
        groups[match.group(1).lower()].append(match.group(3).strip())
    else:
        groups["other"].append(subject)

out = []
for key, heading in (
    ("feat", "Features"),
    ("fix", "Fixes"),
    ("perf", "Performance"),
    ("docs", "Documentation"),
    ("other", "Other changes"),
):
    items = groups[key]
    if not items:
        continue
    out.append(f"### {heading}")
    out.append("")
    out.extend(f"- {item}" for item in items)
    out.append("")

print("\n".join(out).rstrip())
PY
}

if [[ -f "$CHANGELOG_FILE" ]]; then
    BODY="$(render_changelog_json)"
else
    BODY="$(render_commit_fallback)"
fi

if [[ -n "$BODY" ]]; then
    printf '%s\n\n' "$BODY"
fi

if [[ -n "$PREV_TAG" ]]; then
    printf '**Full changelog**: https://github.com/%s/compare/%s...%s\n' \
        "$REPO" "$PREV_TAG" "$TAG"
else
    printf '**Full changelog**: https://github.com/%s/commits/%s\n' "$REPO" "$TAG"
fi
