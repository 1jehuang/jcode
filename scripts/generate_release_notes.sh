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

# Prefer the published release index: replayed history can make the nearest
# ancestor tag much older than the actual preceding release. This also works
# while preparing notes before the new tag exists.
if [[ -z "$PREV_TAG" ]]; then
    PREV_TAG="$(python3 - "$VERSION_NUM" << 'PY'
import json
import pathlib
import re
import sys

def version(value):
    if re.fullmatch(r"\d+\.\d+\.\d+", value):
        return tuple(map(int, value.split(".")))
    return None

current = version(sys.argv[1])
index = pathlib.Path("changelog/index.json")
if current and index.exists():
    entries = json.loads(index.read_text())["entries"]
    candidates = [entry["version"] for entry in entries
                  if version(entry["version"]) is not None
                  and version(entry["version"]) < current]
    if candidates:
        print("v" + max(candidates, key=version))
PY
)"
    if [[ -n "$PREV_TAG" ]] && ! git rev-parse -q --verify "refs/tags/$PREV_TAG" >/dev/null; then
        PREV_TAG=""
    fi
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
