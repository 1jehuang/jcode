#!/usr/bin/env python3
"""Apply the sole allowlisted release repair without changing Git identity."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

PATH = "crates/jcode-companion/src/server.rs"
ORIGINAL_SHA256 = "e4215aa2a36fea7fadfb10235c47ecf34e91a37227e5657d8e909a44994cb2fe"
REPAIRED_SHA256 = "40ccfc4c74f15d013f7fc41b16f7509aa808c0b7f7576f038c53f8568954a1e6"
FIX_COMMIT = "d4f4c8c8d25c9f8e062773683adcc0c19cf26144"
REPAIR = "windows-listener"
ORIGINAL = b"    let listener = bind(&options.socket)?;\n"
REPLACEMENT = ORIGINAL + b"    // Windows named-pipe acceptance replaces the listening pipe instance.\n    #[cfg(windows)]\n    let mut listener = listener;\n"


def git(*args):
    return subprocess.check_output(["git", *args]).decode().strip()


def apply(tag, repair, workflow_sha):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError("Release tag must be vMAJOR.MINOR.PATCH")
    if not re.fullmatch(r"[0-9a-f]{40}", workflow_sha):
        raise ValueError("Workflow commit must be a full SHA")
    commit = git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
    if git("rev-parse", "HEAD") != commit:
        raise ValueError("Checkout must remain at the release tag commit")
    if git("status", "--porcelain", "--untracked-files=no"):
        raise ValueError("Tracked checkout must be clean before recovery")
    manifest = subprocess.check_output(["git", "show", "HEAD:Cargo.toml"]).decode()
    package = manifest.split("[package]", 1)[1].split("[", 1)[0]
    version = re.search(r'^version\s*=\s*"([^"\n]+)"', package, re.MULTILINE)
    if version is None or version.group(1) != tag[1:]:
        raise ValueError("Tag does not match the package version")
    provenance = {"tag": tag, "tag_commit": commit, "workflow_commit": workflow_sha,
                  "repair": repair, "run_url": os.environ.get('RELEASE_RUN_URL', '')}
    provenance["eligibility_policy"] = (
        "exact-sha-main-ci" if repair == "none"
        else "allowlisted-repair-native-companion-preflight"
    )
    if repair != "none":
        if repair != REPAIR or tag not in ("v0.96.0", "v0.97.0"):
            raise ValueError("Repair is not allowlisted for this tag")
        path = Path(PATH)
        original = subprocess.check_output(["git", "show", f"HEAD:{PATH}"])
        if path.read_bytes().replace(b"\r\n", b"\n") != original:
            raise ValueError("Working source differs from canonical tagged source")
        if hashlib.sha256(original).hexdigest() != ORIGINAL_SHA256 or original.count(ORIGINAL) != 1:
            raise ValueError("Original source does not match the reviewed repair")
        repaired = original.replace(ORIGINAL, REPLACEMENT)
        if hashlib.sha256(repaired).hexdigest() != REPAIRED_SHA256:
            raise ValueError("Repaired source does not match the reviewed repair")
        subprocess.run(["git", "merge-base", "--is-ancestor", FIX_COMMIT, workflow_sha], check=True)
        committed = subprocess.check_output(["git", "show", f"{FIX_COMMIT}:{PATH}"])
        if hashlib.sha256(committed).hexdigest() != REPAIRED_SHA256:
            raise ValueError("Committed repair source differs from reviewed content")
        path.write_bytes(repaired)
        if git("diff", "--name-only") != PATH or git("rev-parse", "HEAD") != commit:
            raise ValueError("Recovery modified unexpected source or Git identity")
        provenance.update(fix_commit=FIX_COMMIT, path=PATH, original_sha256=ORIGINAL_SHA256, repaired_sha256=REPAIRED_SHA256)
    # Release-only tooling comes from the reviewed workflow commit, not a branch
    # tip. Older tags predate this helper. It is not Rust application source.
    provenance["release_helpers"] = {}
    for helper in ("scripts/install_justrust_ci.sh", "scripts/build_linux_compat.sh"):
        content = subprocess.check_output(["git", "show", f"{workflow_sha}:{helper}"])
        Path(helper).write_bytes(content)
        Path(helper).chmod(0o755)
        provenance["release_helpers"][helper] = hashlib.sha256(content).hexdigest()
    Path(os.environ.get("RELEASE_PROVENANCE_PATH", "release-provenance.json")).write_text(json.dumps(provenance, indent=2) + "\n")
    return provenance


def check_public_state(tag):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError("Invalid release tag")
    draft = subprocess.check_output(["gh", "release", "view", tag, "--json", "isDraft", "--jq", ".isDraft"]).decode().strip()
    if draft != "true":
        raise ValueError("Recovery requires an existing draft release")
    releases = json.loads(subprocess.check_output(["gh", "release", "list", "--exclude-drafts", "--limit", "1000", "--json", "tagName"]))
    def version(name):
        return tuple(map(int, name[1:].split(".")))
    if any(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", item["tagName"]) and version(item["tagName"]) > version(tag) for item in releases):
        raise ValueError("Refusing recovery that would downgrade a newer public release")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("repair", choices=("none", REPAIR), nargs="?", default="none")
    parser.add_argument("workflow_sha", nargs="?")
    parser.add_argument("--check-public-state", action="store_true")
    args = parser.parse_args()
    if args.check_public_state:
        check_public_state(args.tag)
    elif args.workflow_sha is None:
        parser.error("workflow_sha is required to prepare release source")
    else:
        apply(args.tag, args.repair, args.workflow_sha)
