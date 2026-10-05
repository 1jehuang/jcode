#!/usr/bin/env python3
"""Run every budget ratchet in one pass and report all failures together.

Why this exists
---------------
CI ran the budget ratchets as six separate sequential steps in the `quality`
job (steps 8, 9, 10, 13, 15 and 17 at the time of writing). GitHub Actions stops
a job at its first failing step, so when more than one ratchet is red, a
developer only ever sees one of them. That is not hypothetical: all four
non-cwd ratchets had been red for months behind a single formatting failure at
step 3, so none of them were evaluated at all and nobody knew the total.

`scripts/check_guardrails.sh` already aggregated the gates, but it is a bash
script, which makes it unusable on a Windows checkout, and it mixes the slow
`cargo` gates in with the fast Python ratchets so it is a poor fit for a CI step
that should stay cheap.

This script keeps one result line per ratchet -- so a red gate is still
attributable to a named gate -- while removing the early exit that hid three of
four failures behind the first one.

It does not replace the individual scripts or their baselines. Each ratchet
remains independently runnable, and `check_guardrails.sh --fix` still rebaselines
them; this only changes how the results are *reported*.

Usage:
    python3 scripts/check_all_budgets.py            # check, non-zero on failure
    python3 scripts/check_all_budgets.py --update   # rebaseline, then check
    python3 scripts/check_all_budgets.py --list     # print gate names, run nothing
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_DIR = REPO_ROOT / "scripts"

# Order is the order ci.yml ran them in, so output reads the same as the old
# step list. Every one of these supports --update.
RATCHET_SCRIPTS: tuple[str, ...] = (
    "check_code_size_budget.py",
    "check_test_size_budget.py",
    "check_panic_budget.py",
    "check_cwd_fallback_budget.py",
    "check_swallowed_error_budget.py",
    "check_wildcard_reexport_budget.py",
)

# Output beyond this is truncated per gate, to keep a CI log readable when four
# ratchets are red. The full output is not lost: each ratchet is still runnable
# on its own.
MAX_REPORTED_LINES = 20


@dataclass(frozen=True)
class Guard:
    """One ratchet to invoke."""

    name: str
    script: Path

    def command(self, update: bool) -> list[str]:
        argv = [sys.executable, str(self.script)]
        if update:
            argv.append("--update")
        return argv


@dataclass(frozen=True)
class Result:
    guard: Guard
    returncode: int
    output: str

    @property
    def passed(self) -> bool:
        return self.returncode == 0


def guard_name(script: Path) -> str:
    """`check_panic_budget.py` -> `panic-prone usage`.

    Readable in a log, and stable enough to grep for. Deliberately derived from
    the filename rather than hardcoded, so a renamed ratchet script shows up as
    a new gate name instead of silently inheriting an old label.
    """
    stem = script.stem
    if not stem.startswith("check_") or not stem.endswith("_budget"):
        raise ValueError(
            f"unexpected ratchet filename {script.name!r}; "
            "expected check_<name>_budget.py so guard_name() can derive a label"
        )
    slug = stem[len("check_"): -len("_budget")]
    return slug.replace("_", " ")


def guards(script_dir: Path | None = None) -> list[Guard]:
    """The ratchets to run, in ci.yml order.

    `script_dir` defaults to the sibling scripts directory at call time rather
    than at import time, so a caller (or a test) can point it elsewhere.

    Raises if a script is missing rather than silently running a subset: a guard
    that quietly stops being checked is the exact failure mode this script was
    written to prevent.
    """
    if script_dir is None:
        script_dir = SCRIPT_DIR
    built: list[Guard] = []
    for filename in RATCHET_SCRIPTS:
        path = script_dir / filename
        if not path.is_file():
            raise FileNotFoundError(
                f"ratchet script missing: {path}\n"
                "Refusing to run a partial set; a budget that is not checked "
                "is not a budget."
            )
        built.append(Guard(guard_name(path), path))
    return built


def _tail(text: str, limit: int = MAX_REPORTED_LINES) -> str:
    lines = text.rstrip().splitlines()
    if len(lines) <= limit:
        return "\n".join(lines)
    dropped = len(lines) - limit
    return "\n".join(
        [f"... ({dropped} earlier line(s) omitted)"] + lines[-limit:]
    )


def run_one(guard: Guard, update: bool) -> Result:
    """Run a single ratchet, never raising on a non-zero exit."""
    try:
        proc = subprocess.run(
            guard.command(update),
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as exc:
        return Result(guard, returncode=127, output=f"failed to launch: {exc}")
    return Result(
        guard, proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    )


def run_all(
    guard_list: Sequence[Guard] | None = None, update: bool = False
) -> list[Result]:
    """Run every guard and return one Result each.

    Deliberately does not stop on the first failure: that early exit is the bug
    this script replaces. Every guard runs on every invocation.
    """
    if guard_list is None:
        guard_list = guards()
    return [run_one(g, update) for g in guard_list]


def report(results: Iterable[Result], stream=None) -> list[Result]:
    """Print per-guard PASS/FAIL lines; return the failures.

    `stream` is resolved at call time. A default of `stream=sys.stdout` would
    bind the interpreter's original stdout at import time, so
    `contextlib.redirect_stdout` and a captured CI log would both come up empty
    while the lines went to the real console.
    """
    if stream is None:
        stream = sys.stdout
    failures = [r for r in results if not r.passed]
    for res in results:
        status = "PASS" if res.passed else "FAIL"
        print(f"{status}  {res.guard.name}", file=stream)
        if not res.passed:
            body = _tail(res.output)
            if body:
                for line in body.splitlines():
                    print(f"      {line}", file=stream)
    return failures


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--update",
        action="store_true",
        help="refresh every baseline before checking (intentional growth only)",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="print the gate names and exit without running them",
    )
    args = parser.parse_args(argv)

    try:
        guard_list = guards()
    except FileNotFoundError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    if args.list:
        for guard in guard_list:
            print(guard.name)
        return 0

    results = run_all(guard_list, update=args.update)
    failures = report(results)

    total = len(results)
    if failures:
        print(
            f"\n{len(failures)} of {total} budget gate(s) failed: "
            + ", ".join(r.guard.name for r in failures),
            file=sys.stderr,
        )
        print(
            "Each gate is also runnable on its own; see "
            "scripts/check_guardrails.sh --fix for rebaselining.",
            file=sys.stderr,
        )
        return 1

    print(f"\nAll {total} budget gates passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())