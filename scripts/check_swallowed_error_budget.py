#!/usr/bin/env python3
"""Enforce a ratcheting budget for swallowed-error-like Rust patterns.

This is intentionally a broad guardrail. It tracks production occurrences of
patterns that commonly hide failures and should either be removed, logged,
propagated, or explicitly accepted as best-effort:

- `let _ = ...`
- `.ok()`
- `.unwrap_or_default()`

Policy:
- Existing files may not increase their count.
- New production files may not introduce these patterns.
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
BASELINE_FILE = REPO_ROOT / "scripts" / "swallowed_error_budget.json"
SCAN_ROOTS = (REPO_ROOT / "src", REPO_ROOT / "crates")
PATTERNS = {
    "let_underscore": re.compile(r"\blet\s+_\s*="),
    "dot_ok": re.compile(r"\.ok\(\)"),
    "unwrap_or_default": re.compile(r"\.unwrap_or_default\(\)"),
}
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


def zero_counts() -> dict[str, int]:
    return {name: 0 for name in PATTERNS}


def current_counts() -> dict[str, dict[str, int]]:
    counts: dict[str, dict[str, int]] = {}
    for path in production_rust_files():
        file_counts = zero_counts()
        for line in production_lines(path):
            for name, pattern in PATTERNS.items():
                if pattern.search(line):
                    file_counts[name] += 1
        if sum(file_counts.values()) > 0:
            counts[path.relative_to(REPO_ROOT).as_posix()] = file_counts
    return counts


def file_total(counts: dict[str, int]) -> int:
    return sum(counts.values())


def total_counts(counts: dict[str, dict[str, int]]) -> dict[str, int]:
    totals = zero_counts()
    for file_counts in counts.values():
        for name, count in file_counts.items():
            totals[name] = totals.get(name, 0) + count
    return totals


def grand_total(counts: dict[str, dict[str, int]]) -> int:
    return sum(file_total(file_counts) for file_counts in counts.values())


def load_baseline() -> dict[str, Any]:
    if not BASELINE_FILE.exists():
        return {"version": 1, "total": 0, "totals_by_pattern": zero_counts(), "tracked_files": {}}
    data = json.loads(BASELINE_FILE.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise SystemExit(f"error: invalid baseline file format: {BASELINE_FILE}")
    tracked = data.get("tracked_files")
    totals_by_pattern = data.get("totals_by_pattern")
    total = data.get("total")
    if not isinstance(total, int) or total < 0:
        raise SystemExit(f"error: invalid total in {BASELINE_FILE}")
    if not isinstance(totals_by_pattern, dict):
        raise SystemExit(f"error: invalid totals_by_pattern in {BASELINE_FILE}")
    if not isinstance(tracked, dict):
        raise SystemExit(f"error: invalid tracked_files in {BASELINE_FILE}")
    for path, file_counts in tracked.items():
        if not isinstance(path, str) or not isinstance(file_counts, dict):
            raise SystemExit(f"error: invalid tracked_files entry in {BASELINE_FILE}")
        if any(not isinstance(v, int) or v < 0 for v in file_counts.values()):
            raise SystemExit(f"error: invalid count in tracked_files entry for {path}")
    return data


def write_baseline(counts: dict[str, dict[str, int]]) -> None:
    BASELINE_FILE.write_text(
        json.dumps(
            {
                "version": 1,
                "total": grand_total(counts),
                "totals_by_pattern": total_counts(counts),
                "tracked_files": counts,
            },
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
    current_total = grand_total(current)
    current_pattern_totals = total_counts(current)

    if args.update:
        write_baseline(current)
        print(
            "Updated swallowed-error baseline: "
            f"total={baseline['total']} -> {current_total}, "
            f"files={len(baseline['tracked_files'])} -> {len(current)}"
        )
        return 0

    tracked: dict[str, dict[str, int]] = baseline["tracked_files"]
    regressions: list[str] = []
    improvements: list[str] = []

    if current_total > baseline["total"]:
        regressions.append(f"total swallowed-error-like count grew: {baseline['total']} -> {current_total}")
    elif current_total < baseline["total"]:
        improvements.append(f"total swallowed-error-like count shrank: {baseline['total']} -> {current_total}")

    baseline_pattern_totals: dict[str, int] = baseline["totals_by_pattern"]
    for name, count in sorted(current_pattern_totals.items()):
        old_count = baseline_pattern_totals.get(name, 0)
        if count > old_count:
            regressions.append(f"{name} count grew: {old_count} -> {count}")
        elif count < old_count:
            improvements.append(f"{name} count shrank: {old_count} -> {count}")

    for path, file_counts in sorted(current.items()):
        old_counts = tracked.get(path)
        if old_counts is None:
            regressions.append(f"new swallowed-error-like usage: {path} ({file_total(file_counts)})")
            continue
        old_total = file_total(old_counts)
        new_total = file_total(file_counts)
        if new_total > old_total:
            regressions.append(f"swallowed-error-like usage grew: {path} ({old_total} -> {new_total})")
        elif new_total < old_total:
            improvements.append(f"swallowed-error-like usage shrank: {path} ({old_total} -> {new_total})")

    for path, old_counts in sorted(tracked.items()):
        if path not in current:
            improvements.append(f"swallowed-error-like usage removed: {path} ({file_total(old_counts)} -> 0)")

    if regressions:
        print("Swallowed-error budget exceeded:", file=sys.stderr)
        for entry in regressions:
            print(f"  - {entry}", file=sys.stderr)
        print("Run scripts/check_swallowed_error_budget.py --update only after intentional cleanup.", file=sys.stderr)
        return 1

    if improvements:
        print("Swallowed-error budget improved:")
        for entry in improvements:
            print(f"  - {entry}")
        print("Consider running: scripts/check_swallowed_error_budget.py --update")
    else:
        print(
            "Swallowed-error budget OK: "
            f"total={current_total} files={len(current)} patterns={current_pattern_totals}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
