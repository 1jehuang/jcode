#!/usr/bin/env python3
"""Refresh nix/release.json from a public stable release, using only stdlib."""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import urllib.request


REPOSITORY = "1jehuang/jcode"
ARCHIVES = {
    "x86_64-linux": "jcode-linux-x86_64.tar.gz",
    "aarch64-linux": "jcode-linux-aarch64.tar.gz",
    "x86_64-darwin": "jcode-macos-x86_64.tar.gz",
    "aarch64-darwin": "jcode-macos-aarch64.tar.gz",
}
TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
CHECKSUM = re.compile(r"([0-9a-fA-F]{64}) [ *]([^\s/\\]+)")


def version_tuple(tag: str) -> tuple[int, int, int]:
    match = TAG.fullmatch(tag) if isinstance(tag, str) else None
    if not match:
        raise ValueError("Release tag must be exactly vN.N.N without leading zeroes")
    return tuple(int(part) for part in match.groups())


def asset_url(tag: str, name: str) -> str:
    version_tuple(tag)
    if name not in {*ARCHIVES.values(), "SHA256SUMS"}:
        raise ValueError("Unknown release asset")
    return f"https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"


def fetch_release(tag: str) -> dict:
    version_tuple(tag)
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "jcode-nix-release-updater",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token := os.environ.get("GH_TOKEN"):
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        f"https://api.github.com/repos/{REPOSITORY}/releases/tags/{tag}",
        headers=headers,
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def fetch_checksums(tag: str) -> str:
    # Public download redirects must never receive the API credential.
    request = urllib.request.Request(
        asset_url(tag, "SHA256SUMS"),
        headers={"User-Agent": "jcode-nix-release-updater"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read().decode("utf-8")


def validate_release(release: dict, tag: str) -> None:
    if (
        not isinstance(release, dict)
        or release.get("tag_name") != tag
        or release.get("draft") is not False
        or release.get("prerelease") is not False
        or not release.get("published_at")
    ):
        raise ValueError("Expected a published, non-draft, non-prerelease release for this tag")
    assets = release.get("assets")
    if not isinstance(assets, list):
        raise ValueError("Release assets must be a list")
    names = set()
    required = {*ARCHIVES.values(), "SHA256SUMS"}
    for asset in assets:
        if not isinstance(asset, dict) or not isinstance(asset.get("name"), str):
            raise ValueError("Invalid release asset")
        name = asset["name"]
        if name in names:
            raise ValueError(f"Duplicate release asset: {name}")
        names.add(name)
        if name in required and (
            asset.get("state") != "uploaded"
            or not isinstance(asset.get("size"), int)
            or asset["size"] <= 0
        ):
            raise ValueError(f"Release asset is not uploaded or is empty: {name}")
    if missing := required - names:
        raise ValueError(f"Missing release assets: {', '.join(sorted(missing))}")


def parse_checksums(text: str) -> dict[str, str]:
    digests = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        match = CHECKSUM.fullmatch(line)
        if not match:
            raise ValueError("Malformed SHA256SUMS entry")
        digest, name = match.groups()
        if name in digests:
            raise ValueError(f"Duplicate SHA256SUMS entry: {name}")
        digests[name] = digest
    if missing := set(ARCHIVES.values()) - digests.keys():
        raise ValueError(f"Missing SHA256SUMS entries: {', '.join(sorted(missing))}")
    return {
        system: "sha256-" + base64.b64encode(bytes.fromhex(digests[name])).decode("ascii")
        for system, name in ARCHIVES.items()
    }


def existing_version(metadata: dict) -> tuple[int, int, int]:
    if not isinstance(metadata, dict) or set(metadata) != {"version", "tag", "hashes"}:
        raise ValueError("Invalid existing release metadata schema")
    version = version_tuple(metadata["tag"])
    if metadata["version"] != metadata["tag"][1:]:
        raise ValueError("Existing metadata version and tag disagree")
    hashes = metadata["hashes"]
    if not isinstance(hashes, dict) or hashes.keys() != ARCHIVES.keys():
        raise ValueError("Existing metadata must contain all four platform hashes")
    for value in hashes.values():
        if not isinstance(value, str) or not value.startswith("sha256-"):
            raise ValueError("Invalid existing SRI hash")
        decoded = base64.b64decode(value[7:], validate=True)
        if len(decoded) != 32 or base64.b64encode(decoded).decode("ascii") != value[7:]:
            raise ValueError("Invalid existing SRI hash")
    return version


def update_release(tag: str, path: Path) -> bool:
    version = version_tuple(tag)
    previous = None
    if path.exists():
        previous = json.loads(path.read_text(encoding="utf-8"))
        if version < existing_version(previous):
            raise ValueError("Refusing to downgrade existing release metadata")
    release = fetch_release(tag)
    validate_release(release, tag)
    metadata = {
        "version": tag[1:],
        "tag": tag,
        "hashes": parse_checksums(fetch_checksums(tag)),
    }
    if previous == metadata:
        return False
    payload = json.dumps(metadata, indent=2) + "\n"
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=path.parent, prefix=f".{path.name}.", delete=False
        ) as output:
            temporary = Path(output.name)
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
            os.fchmod(output.fileno(), 0o644)
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--output", type=Path, default=Path("nix/release.json"))
    args = parser.parse_args()
    try:
        changed = update_release(args.tag, args.output)
    except (OSError, ValueError) as error:
        print(f"Nix release metadata update failed: {error}", file=sys.stderr)
        return 1
    print(f"{'Updated' if changed else 'Already current'}: {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
