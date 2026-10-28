#!/usr/bin/env python3
"""Ratchet: no new raw `env::var("...KEY|TOKEN|SECRET...")` reads (issue #1389).

Credentials live in env files and are read through `Creds` / the
`load_*_from_env_or_config` helpers at use time. A raw `env::var` read of a
secret-looking name only sees the process env and silently misses saved keys.

`JCODE_*` names are exempt (config such as keybindings, not secrets).
Existing offenders are tracked in scripts/secret_env_reads_baseline.json and
may not grow. `--update` refreshes the baseline after intentional cleanup.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_panic_budget import REPO_ROOT, production_lines, production_rust_files  # noqa: E402

BASELINE_FILE = REPO_ROOT / "scripts" / "secret_env_reads_baseline.json"
PATTERN = re.compile(r'env::var(?:_os)?\(\s*"(?!JCODE_)([A-Z0-9_]*(?:KEY|TOKEN|SECRET)[A-Z0-9_]*)"')


def current_counts() -> dict[str, int]:
    counts: dict[str, int] = {}
    for path in production_rust_files():
        n = sum(1 for line in production_lines(path) if PATTERN.search(line))
        if n:
            counts[path.relative_to(REPO_ROOT).as_posix()] = n
    return counts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update", action="store_true")
    args = parser.parse_args()
    counts = current_counts()
    if args.update:
        BASELINE_FILE.write_text(json.dumps(counts, indent=2, sort_keys=True) + "\n")
        print(f"baseline updated: {sum(counts.values())} reads in {len(counts)} files")
        return 0
    baseline = json.loads(BASELINE_FILE.read_text()) if BASELINE_FILE.exists() else {}
    bad = [(f, n, baseline.get(f, 0)) for f, n in counts.items() if n > baseline.get(f, 0)]
    if bad:
        for f, n, b in bad:
            print(f"error: {f}: {n} raw secret env reads (baseline {b}); use Creds::get / load_api_key_from_env_or_config")
        return 1
    print(f"secret env read ratchet ok ({sum(counts.values())} legacy reads)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
