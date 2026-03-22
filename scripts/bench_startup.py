#!/usr/bin/env python3
"""Benchmark and optionally regression-check jcode startup time.

This script runs isolated startup measurements under a temporary JCODE_HOME and
JCODE_RUNTIME_DIR so it does not interfere with the user's real server, logs, or
credentials.

Cold client startup is measured by launching the normal default client path in a
pseudo-terminal, then parsing the built-in startup profile written to the
isolated log.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

PROFILE_TOTAL_RE = re.compile(r"Startup Profile \(([0-9.]+)ms total\)")
PROFILE_LINE_RE = re.compile(
    r"\[INFO\]\s+([0-9.]+)ms\s+([0-9.]+)ms\s+[0-9.]+%\s+([a-zA-Z0-9_]+)"
)
REMOTE_HISTORY_RE = re.compile(r"remote bootstrap: history after ([0-9.]+)ms")


@dataclass
class StartupProfile:
    total_ms: float
    deltas_ms: dict[str, float]
    remote_history_ms: float | None = None


@dataclass
class Budget:
    name: str
    actual_ms: float
    limit_ms: float


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="./target/release/jcode")
    parser.add_argument("--runs", type=int, default=5, help="number of startup runs")
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if startup budgets are exceeded",
    )
    parser.add_argument("--max-help-ms", type=float, default=20.0)
    parser.add_argument("--max-version-ms", type=float, default=20.0)
    parser.add_argument("--max-server-ready-ms", type=float, default=80.0)
    parser.add_argument("--max-cold-total-ms", type=float, default=150.0)
    parser.add_argument("--max-cold-server-check-ms", type=float, default=20.0)
    parser.add_argument("--max-cold-server-spawn-ms", type=float, default=20.0)
    parser.add_argument("--max-cold-app-new-ms", type=float, default=20.0)
    parser.add_argument("--max-remote-bootstrap-history-ms", type=float, default=250.0)
    return parser.parse_args()


def median(values: Iterable[float]) -> float:
    vals = list(values)
    if not vals:
        raise ValueError("no values")
    return statistics.median(vals)


def print_stats(name: str, times: list[float]) -> None:
    if not times:
        print(f"\n{name}: No successful runs")
        return
    print(f"\n{name}:")
    print(f"  Min:    {min(times):.2f} ms")
    print(f"  Max:    {max(times):.2f} ms")
    print(f"  Mean:   {statistics.mean(times):.2f} ms")
    print(f"  Median: {statistics.median(times):.2f} ms")
    if len(times) > 1:
        print(f"  Stdev:  {statistics.stdev(times):.2f} ms")


def run_simple_timing(binary: str, *args: str, runs: int) -> list[float]:
    times: list[float] = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run([binary, *args], capture_output=True, check=False)
        times.append((time.perf_counter() - start) * 1000)
    return times


def isolated_env(root: str) -> dict[str, str]:
    env = os.environ.copy()
    env["JCODE_HOME"] = os.path.join(root, "home")
    env["JCODE_RUNTIME_DIR"] = os.path.join(root, "run")
    env["JCODE_SOCKET"] = os.path.join(env["JCODE_RUNTIME_DIR"], "jcode.sock")
    env["JCODE_NO_TELEMETRY"] = "1"
    os.makedirs(env["JCODE_HOME"], exist_ok=True)
    os.makedirs(env["JCODE_RUNTIME_DIR"], exist_ok=True)
    return env


def wait_for_socket(path: str, timeout_s: float) -> bool:
    deadline = time.perf_counter() + timeout_s
    while time.perf_counter() < deadline:
        if os.path.exists(path):
            try:
                sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                sock.connect(path)
                sock.close()
                return True
            except OSError:
                pass
        time.sleep(0.005)
    return False


def measure_server_startup(binary: str, runs: int) -> list[float]:
    times: list[float] = []
    for _ in range(runs):
        root = tempfile.mkdtemp(prefix="jcode-server-bench-")
        env = isolated_env(root)
        socket_path = env["JCODE_SOCKET"]
        proc = None
        try:
            start = time.perf_counter()
            proc = subprocess.Popen(
                [binary, "serve"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env=env,
            )
            if wait_for_socket(socket_path, 5.0):
                times.append((time.perf_counter() - start) * 1000)
        finally:
            if proc is not None:
                proc.terminate()
                try:
                    proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=2)
            shutil.rmtree(root, ignore_errors=True)
    return times


def require_script_binary() -> str:
    script_bin = shutil.which("script")
    if not script_bin:
        raise RuntimeError("'script' utility not found; required for TTY startup benchmark")
    return script_bin


def parse_startup_profile(log_path: Path) -> StartupProfile:
    lines = log_path.read_text().splitlines()
    last_block: list[str] = []
    remote_history_ms = None
    for i, line in enumerate(lines):
        if "=== Startup Profile (" in line:
            last_block = lines[i : i + 40]
        remote_match = REMOTE_HISTORY_RE.search(line)
        if remote_match:
            remote_history_ms = float(remote_match.group(1))

    if not last_block:
        raise RuntimeError(f"no startup profile found in {log_path}")

    total_ms = None
    deltas: dict[str, float] = {}
    for line in last_block:
        total_match = PROFILE_TOTAL_RE.search(line)
        if total_match:
            total_ms = float(total_match.group(1))
        phase_match = PROFILE_LINE_RE.search(line)
        if phase_match:
            _from_start, delta_ms, name = phase_match.groups()
            deltas[name] = float(delta_ms)

    if total_ms is None:
        raise RuntimeError(f"could not parse startup profile total from {log_path}")

    return StartupProfile(
        total_ms=total_ms,
        deltas_ms=deltas,
        remote_history_ms=remote_history_ms,
    )


def measure_cold_client_startup(binary: str, runs: int) -> list[StartupProfile]:
    script_bin = require_script_binary()
    profiles: list[StartupProfile] = []

    for _ in range(runs):
        root = tempfile.mkdtemp(prefix="jcode-cold-bench-")
        env = isolated_env(root)
        log_path = Path(env["JCODE_HOME"]) / "logs" / f"jcode-{time.strftime('%Y-%m-%d')}.log"
        try:
            command = (
                f"{binary} --no-update --debug-socket "
                f"--socket {env['JCODE_SOCKET']}"
            )
            subprocess.run(
                ["timeout", "3s", script_bin, "-qefc", command, "/dev/null"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env=env,
                check=False,
            )
            profiles.append(parse_startup_profile(log_path))
        finally:
            shutil.rmtree(root, ignore_errors=True)

    return profiles


def print_cold_profile_stats(profiles: list[StartupProfile]) -> None:
    totals = [p.total_ms for p in profiles]
    print_stats("Cold client startup total", totals)

    for phase in ["server_check", "server_spawn_start", "server_ready", "app_new_for_remote"]:
        values = [p.deltas_ms[phase] for p in profiles if phase in p.deltas_ms]
        print_stats(f"Cold client phase: {phase}", values)

    remote_history = [p.remote_history_ms for p in profiles if p.remote_history_ms is not None]
    print_stats("Cold client phase: remote bootstrap history", remote_history)


def collect_budgets(
    help_times: list[float],
    version_times: list[float],
    server_times: list[float],
    cold_profiles: list[StartupProfile],
    args: argparse.Namespace,
) -> list[Budget]:
    cold_total = [p.total_ms for p in cold_profiles]
    cold_server_check = [p.deltas_ms.get("server_check", 0.0) for p in cold_profiles]
    cold_server_spawn = [p.deltas_ms.get("server_spawn_start", 0.0) for p in cold_profiles]
    cold_app_new = [p.deltas_ms.get("app_new_for_remote", 0.0) for p in cold_profiles]
    cold_remote_history = [
        p.remote_history_ms for p in cold_profiles if p.remote_history_ms is not None
    ]

    budgets = [
        Budget("--help median", median(help_times), args.max_help_ms),
        Budget("--version median", median(version_times), args.max_version_ms),
        Budget("server ready median", median(server_times), args.max_server_ready_ms),
        Budget("cold startup total median", median(cold_total), args.max_cold_total_ms),
        Budget(
            "cold startup server_check median",
            median(cold_server_check),
            args.max_cold_server_check_ms,
        ),
        Budget(
            "cold startup server_spawn_start median",
            median(cold_server_spawn),
            args.max_cold_server_spawn_ms,
        ),
        Budget(
            "cold startup app_new_for_remote median",
            median(cold_app_new),
            args.max_cold_app_new_ms,
        ),
    ]
    cold_remote_history_median = median_or_none(cold_remote_history)
    if cold_remote_history_median is not None:
        budgets.append(
            Budget(
                "cold startup remote bootstrap history median",
                cold_remote_history_median,
                args.max_remote_bootstrap_history_ms,
            )
        )
    server_ready_median = median_or_none(server_times)
    if server_ready_median is not None:
        budgets[2] = Budget(
            "server ready median",
            server_ready_median,
            args.max_server_ready_ms,
        )
    return budgets


def main() -> int:
    args = parse_args()
    binary = args.binary

    if not os.path.exists(binary):
        print(f"Binary not found: {binary}")
        print("Run: cargo build --release")
        return 1

    print(f"Benchmarking: {binary}")
    print("=" * 60)

    subprocess.run([binary, "--version"], capture_output=True, check=False)

    help_times = run_simple_timing(binary, "--help", runs=args.runs)
    print_stats("--help (binary load)", help_times)

    version_times = run_simple_timing(binary, "--version", runs=args.runs)
    print_stats("--version", version_times)

    print(f"\nMeasuring isolated server startup ({args.runs} runs)...")
    server_times = measure_server_startup(binary, args.runs)
    print_stats("Server ready (isolated socket connectable)", server_times)

    print(f"\nMeasuring isolated cold client startup ({args.runs} runs)...")
    cold_profiles = measure_cold_client_startup(binary, args.runs)
    print_cold_profile_stats(cold_profiles)

    print("\n" + "=" * 60)
    print("Summary:")
    help_median = median_or_none(help_times)
    server_median = median_or_none(server_times)
    cold_median = median_or_none(p.total_ms for p in cold_profiles)
    remote_history_median = median_or_none(
        p.remote_history_ms for p in cold_profiles if p.remote_history_ms is not None
    )
    print(
        f"  Binary load median:      ~{help_median:.1f} ms"
        if help_median is not None
        else "  Binary load median:      n/a"
    )
    print(
        f"  Server ready median:     ~{server_median:.1f} ms"
        if server_median is not None
        else "  Server ready median:     n/a"
    )
    print(
        f"  Cold client total median:~{cold_median:.1f} ms"
        if cold_median is not None
        else "  Cold client total median:n/a"
    )
    print(
        f"  Remote history median:   ~{remote_history_median:.1f} ms"
        if remote_history_median is not None
        else "  Remote history median:   n/a"
    )

    budgets = collect_budgets(help_times, version_times, server_times, cold_profiles, args)
    if args.check:
        failures = [b for b in budgets if b.actual_ms > b.limit_ms]
        print("\nBudget check:")
        for budget in budgets:
            status = "FAIL" if budget.actual_ms > budget.limit_ms else "PASS"
            print(
                f"  [{status}] {budget.name}: {budget.actual_ms:.1f} ms <= {budget.limit_ms:.1f} ms"
            )
        if failures:
            print("\nStartup regression detected.")
            return 2
        print("\nAll startup budgets passed.")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
