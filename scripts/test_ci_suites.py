#!/usr/bin/env python3
"""Run jcode's CI-style test suites with timing and timeout reporting.

This is intentionally split the same way as `.github/workflows/ci.yml` instead of
using one monolithic `cargo test --workspace --all-targets`, which is harder to
interpret locally and can exceed interactive harness command limits. By default
it uses one Rust test thread for deterministic local runs because several tests
exercise process-wide environment and server state; pass `--parallel` to use
Cargo's default test harness parallelism.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from types import FrameType

REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Suite:
    name: str
    timeout_seconds: int
    cargo_args: list[str]

    def command(self, *, parallel: bool) -> list[str]:
        command = ["cargo", *self.cargo_args]
        if not parallel:
            command.extend(["--", "--test-threads=1"])
        return command


SUITES = {
    "lib-bins": Suite("lib-bins", 900, ["test", "--lib", "--bins"]),
    "provider-matrix": Suite(
        "provider-matrix", 600, ["test", "--test", "provider_matrix"]
    ),
    "e2e": Suite("e2e", 900, ["test", "--test", "e2e"]),
}

CURRENT_PROC: subprocess.Popen[bytes] | None = None


def terminate_process_group(proc: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=5)
    except ProcessLookupError:
        return
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()


def handle_signal(signum: int, _frame: FrameType | None) -> None:
    if CURRENT_PROC is not None and CURRENT_PROC.poll() is None:
        terminate_process_group(CURRENT_PROC)
    raise SystemExit(128 + signum)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "suite",
        nargs="*",
        choices=["all", *SUITES.keys()],
        default=["all"],
        help="Suite(s) to run. Defaults to all CI-style suites.",
    )
    parser.add_argument(
        "--timeout-scale",
        type=float,
        default=1.0,
        help="Scale each suite timeout, useful for slow local machines.",
    )
    parser.add_argument(
        "--parallel",
        action="store_true",
        help="Use Cargo's default parallel Rust test execution instead of --test-threads=1.",
    )
    return parser.parse_args()


def selected_suites(names: list[str]) -> list[Suite]:
    if not names or "all" in names:
        return list(SUITES.values())
    return [SUITES[name] for name in names]


def progress(message: str, **extra: object) -> None:
    payload = {"kind": "indeterminate", "message": message}
    payload.update(extra)
    print("JCODE_PROGRESS " + json.dumps(payload), flush=True)


def run_suite(suite: Suite, timeout_scale: float, *, parallel: bool) -> tuple[int, float]:
    timeout_seconds = max(1, int(suite.timeout_seconds * timeout_scale))
    started = time.monotonic()
    progress(
        f"Running {suite.name}",
        current=0,
        total=1,
        unit="suite",
        eta_seconds=timeout_seconds,
    )
    command = suite.command(parallel=parallel)
    print(f"\n=== {suite.name} ===", flush=True)
    print("$ " + " ".join(command), flush=True)

    global CURRENT_PROC
    proc = subprocess.Popen(command, cwd=REPO_ROOT, start_new_session=True)
    CURRENT_PROC = proc
    try:
        returncode = proc.wait(timeout=timeout_seconds)
        elapsed = time.monotonic() - started
        print(
            f"=== {suite.name} exit={returncode} elapsed={elapsed:.1f}s timeout={timeout_seconds}s ===",
            flush=True,
        )
        return returncode, elapsed
    except subprocess.TimeoutExpired:
        elapsed = time.monotonic() - started
        terminate_process_group(proc)
        print(
            f"=== {suite.name} timed out after {elapsed:.1f}s timeout={timeout_seconds}s ===",
            flush=True,
        )
        return 124, elapsed
    finally:
        if CURRENT_PROC is proc:
            CURRENT_PROC = None


def main() -> int:
    signal.signal(signal.SIGTERM, handle_signal)
    signal.signal(signal.SIGINT, handle_signal)

    args = parse_args()
    suites = selected_suites(args.suite)
    failures: list[tuple[str, int, float]] = []
    total_started = time.monotonic()

    for index, suite in enumerate(suites, start=1):
        progress(f"Running {suite.name} ({index}/{len(suites)})", current=index, total=len(suites), unit="suite")
        code, elapsed = run_suite(suite, args.timeout_scale, parallel=args.parallel)
        if code != 0:
            failures.append((suite.name, code, elapsed))
            break

    total_elapsed = time.monotonic() - total_started
    print("\n=== test suite summary ===", flush=True)
    print(f"suites={len(suites)} elapsed={total_elapsed:.1f}s", flush=True)
    if failures:
        for name, code, elapsed in failures:
            print(f"FAILED {name}: exit={code} elapsed={elapsed:.1f}s", flush=True)
        return failures[0][1]

    print("All selected CI-style test suites passed.", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
