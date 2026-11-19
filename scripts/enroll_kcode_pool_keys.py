#!/usr/bin/env python3
"""Emit secret-free kcode pool entries from a dotenv-style KEY=value file.

Usage:
  python scripts/enroll_kcode_pool_keys.py <keys-file> --provider <provider> --out <outside-repo-output.json>

The input file is read only. Each value becomes one tier-3 API-key pool
member with enrolled=true. Values are hashed into ids and are never printed or
written. The output path must be outside this repository working tree.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Tuple

PROVIDER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
TIER = 3


class HelperError(Exception):
    """A validation failure that never includes credential material."""


def parse_args(argv: List[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Enrol API keys as secret-free kcode pool entries.",
    )
    parser.add_argument("keys_file", help="dotenv-style KEY=value input file")
    parser.add_argument("--provider", required=True, help="single provider id for every entry")
    parser.add_argument("--out", required=True, help="output JSON path outside this repository")
    return parser.parse_args(argv)


def repo_root(script_path: Path) -> Path:
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        cwd=str(script_path.resolve().parent),
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise HelperError("cannot determine repository root")
    return Path(result.stdout.strip()).resolve()


def path_within(path: Path, root: Path) -> bool:
    path_key = os.path.normcase(os.path.normpath(str(path)))
    root_key = os.path.normcase(os.path.normpath(str(root)))
    try:
        return os.path.commonpath([path_key, root_key]) == root_key
    except ValueError:
        return False


def read_values(keys_file: Path, provider: str) -> Tuple[List[Dict[str, object]], int]:
    try:
        text = keys_file.read_text(encoding="utf-8")
    except OSError as exc:
        raise HelperError(f"cannot read input file: {exc.strerror or 'I/O error'}") from None
    except UnicodeDecodeError:
        raise HelperError("input file must be UTF-8 text") from None

    entries: List[Dict[str, object]] = []
    seen: set[str] = set()
    duplicates = 0

    for lineno, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.lstrip().startswith("export "):
            line = line.lstrip()[7:].lstrip()
        name, separator, value = line.partition("=")
        if not separator or not name.strip():
            raise HelperError(f"line {lineno}: expected KEY=value")
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        if not value:
            raise HelperError(f"line {lineno}: empty value")

        digest = hashlib.sha256(f"{provider}\0{value}".encode("utf-8")).hexdigest()[:12]
        output_hash = hashlib.sha256(value.encode("utf-8")).hexdigest()[:12]
        if digest in seen:
            duplicates += 1
            continue
        seen.add(digest)
        entries.append(
            {
                "id": f"{provider}-key-{len(entries) + 1:02d}-{output_hash}",
                "provider": provider,
                "tier": TIER,
                "kind": "ApiKey",
                "enrolled": True,
            }
        )

    return entries, duplicates


def main(argv: List[str]) -> int:
    args = parse_args(argv)
    if not PROVIDER_RE.match(args.provider):
        raise HelperError("provider must match [A-Za-z0-9][A-Za-z0-9._-]*")

    keys_file = Path(args.keys_file).expanduser().resolve()
    if not keys_file.is_file():
        raise HelperError("input path is not a file")

    out_path = Path(args.out).expanduser().resolve()
    root = repo_root(Path(__file__))
    if out_path == root or path_within(out_path, root):
        raise HelperError("output path must be outside the repository working tree")
    if not out_path.parent.is_dir():
        raise HelperError("output parent directory does not exist")
    if out_path.is_dir():
        raise HelperError("output path is a directory")

    entries, duplicates = read_values(keys_file, args.provider)
    if not entries:
        raise HelperError("no KEY=value entries found")

    out_path.write_text(json.dumps({"entries": entries}, indent=2) + "\n", encoding="utf-8")
    try:
        os.chmod(out_path, 0o600)
    except OSError:
        pass

    print(f"wrote {len(entries)} secret-free pool entries to {out_path}")
    if duplicates:
        print(f"skipped {duplicates} duplicate value(s)")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except HelperError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(2)
