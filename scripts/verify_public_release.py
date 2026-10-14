#!/usr/bin/env python3
"""Verify a public release's complete asset set, provenance and downloaded hashes.

Uses authenticated gh CLI access. Never executes or extracts downloaded binaries.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import os

PLATFORMS = ("linux-x86_64", "linux-aarch64", "macos-aarch64", "macos-x86_64",
             "windows-x86_64", "windows-aarch64", "freebsd-x86_64")
REQUIRED = {f"jcode-{platform}.tar.gz" for platform in PLATFORMS} | {
    "jcode-windows-x86_64.exe", "jcode-windows-aarch64.exe",
    "SHA256SUMS", "release-provenance.json",
}


def verify_metadata(metadata, tag):
    if metadata.get("tag_name") != tag or metadata.get("draft") is not False:
        raise ValueError("Release must be public and match the requested tag")
    if metadata.get("prerelease") is not False:
        raise ValueError("Official release must not be a prerelease")
    assets = metadata.get("assets", [])
    names = [asset.get("name") for asset in assets]
    if any(not isinstance(name, str) for name in names) or len(names) != len(set(names)):
        raise ValueError("Invalid or duplicate release asset names")
    missing = REQUIRED - set(names)
    if missing:
        raise ValueError(f"Missing required release assets: {sorted(missing)}")
    for asset in assets:
        if (Path(asset["name"]).name != asset["name"] or asset["name"] in {".", ".."}
                or "\\" in asset["name"] or asset.get("state") != "uploaded"
                or type(asset.get("size")) is not int or asset["size"] <= 0):
            raise ValueError("Release has unsafe, incomplete or empty assets")
    return set(names)


def verify_downloads(directory, names, tag, expected_sha):
    directory = Path(directory)
    checksums = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *]([^/\\]+)", line)
        if not match or match[2] in checksums:
            raise ValueError("Malformed or duplicate checksum entry")
        checksums[match[2]] = match[1].lower()
    if set(checksums) != names - {"SHA256SUMS"}:
        raise ValueError("Checksum manifest must cover exactly the published assets")
    for name in names:
        path = directory / name
        if not path.is_file() or path.is_symlink() or path.stat().st_size == 0:
            raise ValueError(f"Missing or unsafe downloaded asset: {name}")
        if name != "SHA256SUMS":
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            if digest != checksums[name]:
                raise ValueError(f"Checksum mismatch: {name}")
    provenance = json.loads((directory / "release-provenance.json").read_text())
    if (provenance.get("tag") != tag or provenance.get("tag_commit") != expected_sha
            or provenance.get("repair") != "none"
            or provenance.get("eligibility_policy") != "exact-sha-main-ci"):
        raise ValueError("Release provenance does not match the validated unpatched commit")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--repo", default="1jehuang/jcode")
    parser.add_argument("--sha", required=True, help="Validated exact tag commit")
    parser.add_argument("--directory", help="Keep downloads in this new directory")
    args = parser.parse_args()
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", args.tag):
        parser.error("tag must be vMAJOR.MINOR.PATCH")
    if not re.fullmatch(r"[0-9a-f]{40}", args.sha):
        parser.error("sha must be a full lowercase commit SHA")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*", args.repo):
        parser.error("repo must be OWNER/REPO")
    metadata = json.loads(subprocess.check_output(
        ["gh", "api", f"repos/{args.repo}/releases/tags/{args.tag}"]))
    names = verify_metadata(metadata, args.tag)

    def download_and_verify(directory):
        subprocess.run(["gh", "release", "download", args.tag, "--repo", args.repo,
                        "--dir", str(directory)], check=True)
        verify_downloads(directory, names, args.tag, args.sha)
        print(f"VERIFIED: {args.tag}, {len(names)} public assets, exact provenance and SHA256 hashes")

    if args.directory:
        directory = Path(args.directory)
        directory.mkdir(parents=True, exist_ok=False)
        download_and_verify(directory)
    else:
        with tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR")) as directory:
            download_and_verify(directory)


if __name__ == "__main__":
    main()
