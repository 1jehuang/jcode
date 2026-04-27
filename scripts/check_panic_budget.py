#!/usr/bin/env python3
"""Enforce a ratcheting production panic-prone usage budget.

Counts production Rust occurrences of:
- `.unwrap(`
- `.expect(`
- `panic!`, `todo!`, `unimplemented!`

Policy:
- Existing files may not increase their count.
- New production files may not introduce panic-prone usage.
- Total count may not increase.
- `--update` refreshes the baseline after intentional cleanup.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
BASELINE_FILE = REPO_ROOT / "scripts" / "panic_budget.json"
SCAN_ROOTS = (REPO_ROOT / "src", REPO_ROOT / "crates")
PATTERN = re.compile(r"\.unwrap\(|\.expect\(|\b(?:panic!|todo!|unimplemented!)")
CFG_TEST_RE = re.compile(r"^\s*#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
ITEM_START_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:mod|fn)\b")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update", action="store_true", help="refresh the baseline")
    return parser.parse_args()


def is_test_rust_file(path: Path) -> bool:
    rel = path.relative_to(REPO_ROOT).as_posix()
    if path.suffix != ".rs":
        return False
    parts = rel.split("/")
    if parts[0] == "tests" or any(
        part == "tests" or part.endswith("_tests") or part.endswith("_test") or part.startswith("tests_")
        for part in parts
    ):
        return True
    name = path.name
    return (
        name == "tests.rs"
        or name.endswith("_tests.rs")
        or name.endswith("_test.rs")
        or name.startswith("tests_")
    )


def production_rust_files() -> list[Path]:
    files: list[Path] = []
    for root in SCAN_ROOTS:
        if not root.exists():
            continue
        for path in sorted(root.rglob("*.rs")):
            if path.suffix == ".rs" and not is_test_rust_file(path):
                files.append(path)
    return files


def brace_delta(line: str) -> int:
    """Approximate Rust block nesting for budget classification.

    The panic budget is a ratchet, not a parser. This intentionally simple scan
    ignores comments and strings, which is acceptable for excluding normal
    `#[cfg(test)] mod tests { ... }` blocks from production counts.
    """
    return line.count("{") - line.count("}")


def production_lines(path: Path) -> list[str]:
    lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    output: list[str] = []
    skip_stack: list[int] = []
    pending_cfg_test = False

    for line in lines:
        stripped = line.strip()
        current_depth = sum(skip_stack)
        if current_depth == 0:
            if pending_cfg_test and ITEM_START_RE.match(line):
                delta = brace_delta(line)
                if delta > 0:
                    skip_stack.append(delta)
                pending_cfg_test = False
                continue
            if pending_cfg_test and stripped and not stripped.startswith("#"):
                pending_cfg_test = False
            if CFG_TEST_RE.match(line):
                pending_cfg_test = True
                continue
            output.append(line)
        else:
            skip_stack[-1] += brace_delta(line)
            if skip_stack[-1] <= 0:
                skip_stack.pop()
    return output


def current_counts() -> dict[str, int]:
    counts: dict[str, int] = {}
    for path in production_rust_files():
        count = sum(1 for line in production_lines(path) if PATTERN.search(line))
        if count:
            counts[path.relative_to(REPO_ROOT).as_posix()] = count
    return counts


def load_baseline() -> dict[str, Any]:
    if not BASELINE_FILE.exists():
        return {"version": 1, "total": 0, "tracked_files": {}}
    data = json.loads(BASELINE_FILE.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise SystemExit(f"error: invalid baseline file format: {BASELINE_FILE}")
    total = data.get("total")
    tracked = data.get("tracked_files")
    if not isinstance(total, int) or total < 0:
        raise SystemExit(f"error: invalid total in {BASELINE_FILE}")
    if not isinstance(tracked, dict) or any(
        not isinstance(k, str) or not isinstance(v, int) or v <= 0 for k, v in tracked.items()
    ):
        raise SystemExit(f"error: invalid tracked_files in {BASELINE_FILE}")
    return data


def write_baseline(counts: dict[str, int]) -> None:
    BASELINE_FILE.write_text(
        json.dumps(
            {"version": 1, "total": sum(counts.values()), "tracked_files": counts},
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


def main() -> int:
    args = parse_args()
    baseline = load_baseline()
    current = current_counts()
    current_total = sum(current.values())

    if args.update:
        write_baseline(current)
        print(
            "Updated panic-prone baseline: "
            f"total={baseline['total']} -> {current_total}, files={len(baseline['tracked_files'])} -> {len(current)}"
        )
        return 0

    tracked: dict[str, int] = baseline["tracked_files"]
    regressions: list[str] = []
    improvements: list[str] = []

    if current_total > baseline["total"]:
        regressions.append(f"total panic-prone count grew: {baseline['total']} -> {current_total}")
    elif current_total < baseline["total"]:
        improvements.append(f"total panic-prone count shrank: {baseline['total']} -> {current_total}")

    for path, count in sorted(current.items()):
        old_count = tracked.get(path)
        if old_count is None:
            regressions.append(f"new production panic-prone usage: {path} ({count})")
        elif count > old_count:
            regressions.append(f"production panic-prone usage grew: {path} ({old_count} -> {count})")
        elif count < old_count:
            improvements.append(f"production panic-prone usage shrank: {path} ({old_count} -> {count})")

    for path, old_count in sorted(tracked.items()):
        if path not in current:
            improvements.append(f"production panic-prone usage removed: {path} ({old_count} -> 0)")

    if regressions:
        print("Panic-prone usage budget exceeded:", file=sys.stderr)
        for entry in regressions:
            print(f"  - {entry}", file=sys.stderr)
        print("Run scripts/check_panic_budget.py --update only after intentional cleanup.", file=sys.stderr)
        return 1

    if improvements:
        print("Panic-prone usage budget improved:")
        for entry in improvements:
            print(f"  - {entry}")
        print("Consider running: scripts/check_panic_budget.py --update")
    else:
        print(f"Panic-prone budget OK: total={current_total} files={len(current)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
