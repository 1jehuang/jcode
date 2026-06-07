#!/usr/bin/env python3
"""Benchmark + profile the time and resource utilization of a jcode spawn.

A jcode "spawn" is the full cold-launch path of the interactive client:

    process start -> tokio runtime -> startup setup -> arg parse
        -> (cold) spawn the background server daemon -> connect TUI client.

This profiler complements scripts/bench_startup.py (which only measures time)
by also attributing RESOURCE UTILIZATION to each spawned process:

  * wall-clock time (and the built-in startup-profile phase breakdown)
  * peak / final RSS, anon vs file-backed RSS, PSS, swap
  * CPU time (user + system) and average %CPU over the spawn window
  * thread count high-water mark
  * open file descriptors high-water mark
  * minor / major page faults
  * voluntary / involuntary context switches
  * block I/O bytes read/written

Everything runs under an isolated JCODE_HOME / JCODE_RUNTIME_DIR / JCODE_SOCKET
so it never touches the user's real shared server, logs, sessions, or creds.

Two spawn shapes are profiled:

  server : the background daemon (`jcode serve`) on a private socket. This is
           the heavy half of a cold spawn and owns the long-lived footprint.
  client : the cold interactive client launched in a PTY, which itself spawns
           the server when none is running. We sample BOTH the client and the
           daemon it spawns.

Usage:
    python3 scripts/profile_spawn.py [BINARY] [--runs N] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import select
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field, asdict
from pathlib import Path

try:
    import pty
    import termios
    import struct
    import fcntl

    _HAVE_PTY = True
except ImportError:  # pragma: no cover - non-unix
    _HAVE_PTY = False

CLK_TCK = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
PAGE_SIZE = os.sysconf("SC_PAGE_SIZE") if hasattr(os, "sysconf") else 4096

PROFILE_TOTAL_RE = re.compile(r"Startup Profile \(([0-9.]+)ms total\)")
PROFILE_LINE_RE = re.compile(
    r"\[INFO\]\s+([0-9.]+)ms\s+([0-9.]+)ms\s+[0-9.]+%\s+([a-zA-Z0-9_]+)"
)
REMOTE_HISTORY_RE = re.compile(r"remote bootstrap: history after ([0-9.]+)ms")


# --------------------------------------------------------------------------- #
# /proc sampling
# --------------------------------------------------------------------------- #
def _read(path: str) -> str | None:
    try:
        with open(path, "r") as fh:
            return fh.read()
    except OSError:
        return None


def _status_kib(status: str, key: str) -> int | None:
    m = re.search(rf"^{re.escape(key)}\s+(\d+)\s*kB", status, re.MULTILINE)
    return int(m.group(1)) * 1024 if m else None


@dataclass
class ProcSample:
    """One point-in-time sample of a process from /proc/<pid>."""

    rss_bytes: int | None = None
    rss_anon_bytes: int | None = None
    rss_file_bytes: int | None = None
    peak_rss_bytes: int | None = None
    pss_bytes: int | None = None
    swap_bytes: int | None = None
    threads: int | None = None
    fds: int | None = None
    # cumulative counters
    utime_s: float | None = None
    stime_s: float | None = None
    minflt: int | None = None
    majflt: int | None = None
    vol_ctxt: int | None = None
    nonvol_ctxt: int | None = None
    read_bytes: int | None = None
    write_bytes: int | None = None


def sample_proc(pid: int) -> ProcSample | None:
    status = _read(f"/proc/{pid}/status")
    stat = _read(f"/proc/{pid}/stat")
    if status is None or stat is None:
        return None
    s = ProcSample()
    s.rss_bytes = _status_kib(status, "VmRSS:")
    s.rss_anon_bytes = _status_kib(status, "RssAnon:")
    s.rss_file_bytes = _status_kib(status, "RssFile:")
    s.peak_rss_bytes = _status_kib(status, "VmHWM:")
    s.swap_bytes = _status_kib(status, "VmSwap:")
    tm = re.search(r"^Threads:\s+(\d+)", status, re.MULTILINE)
    s.threads = int(tm.group(1)) if tm else None

    rollup = _read(f"/proc/{pid}/smaps_rollup")
    if rollup:
        pm = re.search(r"^Pss:\s+(\d+)\s*kB", rollup, re.MULTILINE)
        s.pss_bytes = int(pm.group(1)) * 1024 if pm else None

    # /proc/<pid>/stat fields after "comm) ": index 0 == field 3 (state),
    # so field N maps to fields[N-3]. minflt=10 majflt=12 utime=14 stime=15.
    rparen = stat.rfind(")")
    if rparen != -1:
        fields = stat[rparen + 2 :].split()
        try:
            s.minflt = int(fields[10 - 3])  # field 10
            s.majflt = int(fields[12 - 3])  # field 12
            s.utime_s = int(fields[14 - 3]) / CLK_TCK  # field 14
            s.stime_s = int(fields[15 - 3]) / CLK_TCK  # field 15
        except (IndexError, ValueError):
            pass

    sched = status
    vm = re.search(r"^voluntary_ctxt_switches:\s+(\d+)", sched, re.MULTILINE)
    nm = re.search(r"^nonvoluntary_ctxt_switches:\s+(\d+)", sched, re.MULTILINE)
    s.vol_ctxt = int(vm.group(1)) if vm else None
    s.nonvol_ctxt = int(nm.group(1)) if nm else None

    io = _read(f"/proc/{pid}/io")
    if io:
        rm = re.search(r"^read_bytes:\s+(\d+)", io, re.MULTILINE)
        wm = re.search(r"^write_bytes:\s+(\d+)", io, re.MULTILINE)
        s.read_bytes = int(rm.group(1)) if rm else None
        s.write_bytes = int(wm.group(1)) if wm else None

    try:
        s.fds = len(os.listdir(f"/proc/{pid}/fd"))
    except OSError:
        s.fds = None
    return s


@dataclass
class ResourceProfile:
    """Aggregated resource utilization for one spawned process over its window."""

    wall_ms: float = 0.0
    peak_rss_bytes: int = 0
    final_rss_bytes: int = 0
    peak_rss_anon_bytes: int = 0
    peak_pss_bytes: int = 0
    peak_swap_bytes: int = 0
    peak_threads: int = 0
    peak_fds: int = 0
    cpu_user_s: float = 0.0
    cpu_sys_s: float = 0.0
    cpu_total_s: float = 0.0
    cpu_pct: float = 0.0
    minflt: int = 0
    majflt: int = 0
    vol_ctxt: int = 0
    nonvol_ctxt: int = 0
    io_read_bytes: int = 0
    io_write_bytes: int = 0
    samples: int = 0


class ProcessMonitor:
    """High-frequency sampler that tracks one pid's resource high-water marks."""

    def __init__(self, pid: int):
        self.pid = pid
        self.first: ProcSample | None = None
        self.last: ProcSample | None = None
        self.peak_rss = 0
        self.peak_rss_anon = 0
        self.peak_pss = 0
        self.peak_swap = 0
        self.peak_threads = 0
        self.peak_fds = 0
        self.start = time.perf_counter()
        self.end = self.start
        self.n = 0

    def poll(self) -> bool:
        s = sample_proc(self.pid)
        if s is None:
            return False
        self.n += 1
        self.end = time.perf_counter()
        if self.first is None:
            self.first = s
        self.last = s
        if s.rss_bytes:
            self.peak_rss = max(self.peak_rss, s.rss_bytes)
        if s.peak_rss_bytes:
            self.peak_rss = max(self.peak_rss, s.peak_rss_bytes)
        if s.rss_anon_bytes:
            self.peak_rss_anon = max(self.peak_rss_anon, s.rss_anon_bytes)
        if s.pss_bytes:
            self.peak_pss = max(self.peak_pss, s.pss_bytes)
        if s.swap_bytes:
            self.peak_swap = max(self.peak_swap, s.swap_bytes)
        if s.threads:
            self.peak_threads = max(self.peak_threads, s.threads)
        if s.fds:
            self.peak_fds = max(self.peak_fds, s.fds)
        return True

    def finalize(self) -> ResourceProfile:
        p = ResourceProfile()
        p.wall_ms = (self.end - self.start) * 1000.0
        p.samples = self.n
        p.peak_rss_bytes = self.peak_rss
        p.peak_rss_anon_bytes = self.peak_rss_anon
        p.peak_pss_bytes = self.peak_pss
        p.peak_swap_bytes = self.peak_swap
        p.peak_threads = self.peak_threads
        p.peak_fds = self.peak_fds
        if self.last:
            p.final_rss_bytes = self.last.rss_bytes or 0
        l = self.last
        if l:
            # Cumulative counters in /proc are totals since process birth. We
            # monitor from spawn, so the final absolute value IS the total
            # resource cost of the spawn (more robust than last-first, which
            # would miss work done before the first sample landed).
            if l.utime_s is not None:
                p.cpu_user_s = l.utime_s
            if l.stime_s is not None:
                p.cpu_sys_s = l.stime_s
            p.cpu_total_s = p.cpu_user_s + p.cpu_sys_s
            if p.wall_ms > 0:
                p.cpu_pct = 100.0 * p.cpu_total_s / (p.wall_ms / 1000.0)
            if l.minflt is not None:
                p.minflt = l.minflt
            if l.majflt is not None:
                p.majflt = l.majflt
            if l.vol_ctxt is not None:
                p.vol_ctxt = l.vol_ctxt
            if l.nonvol_ctxt is not None:
                p.nonvol_ctxt = l.nonvol_ctxt
            if l.read_bytes is not None:
                p.io_read_bytes = l.read_bytes
            if l.write_bytes is not None:
                p.io_write_bytes = l.write_bytes
        return p


# --------------------------------------------------------------------------- #
# Environment isolation
# --------------------------------------------------------------------------- #
def isolated_env(root: str) -> dict[str, str]:
    env = os.environ.copy()
    env["JCODE_HOME"] = os.path.join(root, "home")
    env["JCODE_RUNTIME_DIR"] = os.path.join(root, "run")
    env["JCODE_SOCKET"] = os.path.join(env["JCODE_RUNTIME_DIR"], "jcode.sock")
    env["JCODE_NO_TELEMETRY"] = "1"
    os.makedirs(env["JCODE_HOME"], exist_ok=True)
    os.makedirs(env["JCODE_RUNTIME_DIR"], exist_ok=True)
    return env


def socket_connectable(path: str) -> bool:
    if not os.path.exists(path):
        return False
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(path)
        s.close()
        return True
    except OSError:
        return False


def find_child_server(env: dict[str, str], exclude: set[int]) -> int | None:
    """Find the spawned `serve` daemon bound to our private socket."""
    try:
        pids = [int(p) for p in os.listdir("/proc") if p.isdigit()]
    except OSError:
        return None
    for pid in pids:
        if pid in exclude:
            continue
        cmd = _read(f"/proc/{pid}/cmdline")
        if not cmd:
            continue
        argv = cmd.split("\0")
        if "serve" in argv and any(env["JCODE_SOCKET"] in a or "serve" == a for a in argv):
            # confirm it is our isolated daemon by socket env or socket arg
            envblob = _read(f"/proc/{pid}/environ") or ""
            if env["JCODE_SOCKET"] in envblob or env["JCODE_SOCKET"] in cmd:
                return pid
    return None


# --------------------------------------------------------------------------- #
# Startup profile parsing
# --------------------------------------------------------------------------- #
@dataclass
class StartupProfile:
    total_ms: float = 0.0
    deltas_ms: dict[str, float] = field(default_factory=dict)
    remote_history_ms: float | None = None


def parse_startup_profile(log_path: Path) -> StartupProfile | None:
    if not log_path.exists():
        return None
    lines = log_path.read_text(errors="replace").splitlines()
    last_block: list[str] = []
    remote_history_ms = None
    for i, line in enumerate(lines):
        if "=== Startup Profile (" in line:
            last_block = lines[i : i + 40]
        m = REMOTE_HISTORY_RE.search(line)
        if m:
            remote_history_ms = float(m.group(1))
    if not last_block:
        return None
    prof = StartupProfile(remote_history_ms=remote_history_ms)
    for line in last_block:
        tm = PROFILE_TOTAL_RE.search(line)
        if tm:
            prof.total_ms = float(tm.group(1))
        pm = PROFILE_LINE_RE.search(line)
        if pm:
            _from, delta, name = pm.groups()
            prof.deltas_ms[name] = float(delta)
    return prof


# --------------------------------------------------------------------------- #
# Server spawn profiling
# --------------------------------------------------------------------------- #
def profile_server_spawn(binary: str, sample_interval_s: float) -> dict | None:
    root = tempfile.mkdtemp(prefix="jcode-spawn-server-")
    env = isolated_env(root)
    sock = env["JCODE_SOCKET"]
    proc = None
    try:
        t0 = time.perf_counter()
        proc = subprocess.Popen(
            [binary, "--no-update", "serve"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=env,
        )
        mon = ProcessMonitor(proc.pid)
        ready_ms = None
        deadline = t0 + 8.0
        while time.perf_counter() < deadline:
            if not mon.poll():
                break
            if ready_ms is None and socket_connectable(sock):
                ready_ms = (time.perf_counter() - t0) * 1000.0
                # keep sampling briefly to capture steady-state footprint
                settle_until = time.perf_counter() + 0.4
                while time.perf_counter() < settle_until:
                    mon.poll()
                    time.sleep(sample_interval_s)
                break
            time.sleep(sample_interval_s)
        res = mon.finalize()
        out = asdict(res)
        out["ready_ms"] = ready_ms
        return out
    finally:
        if proc is not None:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)
        shutil.rmtree(root, ignore_errors=True)


# --------------------------------------------------------------------------- #
# Cold client spawn profiling (PTY) - samples both client and its server child
# --------------------------------------------------------------------------- #
def profile_client_spawn(binary: str, sample_interval_s: float, hold_s: float) -> dict | None:
    if not _HAVE_PTY:
        return None
    root = tempfile.mkdtemp(prefix="jcode-spawn-client-")
    env = isolated_env(root)
    log_path = Path(env["JCODE_HOME"]) / "logs" / f"jcode-{time.strftime('%Y-%m-%d')}.log"
    pid = None
    master_fd = None
    try:
        pid, master_fd = pty.fork()
        if pid == 0:  # child
            try:
                # 120x40 window
                winsize = struct.pack("HHHH", 40, 120, 0, 0)
                fcntl.ioctl(0, termios.TIOCSWINSZ, winsize)
            except OSError:
                pass
            os.execve(
                binary,
                [binary, "--no-update", "--socket", env["JCODE_SOCKET"]],
                env,
            )
            os._exit(127)

        # parent: monitor the client and (once spawned) the server daemon
        t0 = time.perf_counter()
        client_mon = ProcessMonitor(pid)
        server_mon = None
        server_pid = None
        server_ready_ms = None
        deadline = t0 + hold_s
        # drain pty output so the child does not block on a full pipe
        while time.perf_counter() < deadline:
            r, _, _ = select.select([master_fd], [], [], 0.0)
            if r:
                try:
                    os.read(master_fd, 65536)
                except OSError:
                    pass
            if not client_mon.poll():
                break
            if server_pid is None:
                cand = find_child_server(env, exclude={pid, os.getpid()})
                if cand is not None:
                    server_pid = cand
                    server_mon = ProcessMonitor(server_pid)
            if server_mon is not None:
                server_mon.poll()
                if server_ready_ms is None and socket_connectable(env["JCODE_SOCKET"]):
                    server_ready_ms = (time.perf_counter() - t0) * 1000.0
            time.sleep(sample_interval_s)

        client_res = asdict(client_mon.finalize())
        server_res = asdict(server_mon.finalize()) if server_mon else None
        profile = parse_startup_profile(log_path)
        return {
            "client": client_res,
            "server": server_res,
            "server_ready_ms": server_ready_ms,
            "startup_profile": asdict(profile) if profile else None,
        }
    finally:
        if pid:
            try:
                os.kill(pid, 15)
            except OSError:
                pass
            time.sleep(0.05)
            try:
                os.kill(pid, 9)
            except OSError:
                pass
            try:
                os.waitpid(pid, 0)
            except OSError:
                pass
        if master_fd is not None:
            try:
                os.close(master_fd)
            except OSError:
                pass
        shutil.rmtree(root, ignore_errors=True)


# --------------------------------------------------------------------------- #
# Aggregation / reporting
# --------------------------------------------------------------------------- #
def mb(n) -> str:
    if not n:
        return "    -"
    return f"{n / (1024 * 1024):6.1f}"


def agg(values: list[float]) -> dict:
    vals = [v for v in values if v is not None]
    if not vals:
        return {"min": None, "median": None, "max": None, "mean": None}
    return {
        "min": min(vals),
        "median": statistics.median(vals),
        "max": max(vals),
        "mean": statistics.mean(vals),
    }


def median_field(runs: list[dict], key: str) -> float | None:
    vals = [r[key] for r in runs if r and r.get(key) is not None]
    return statistics.median(vals) if vals else None


def print_resource_block(title: str, runs: list[dict]) -> None:
    runs = [r for r in runs if r]
    if not runs:
        print(f"\n{title}: no data")
        return
    print(f"\n{title}  (median of {len(runs)} runs)")
    rows = [
        ("Peak RSS (MB)", "peak_rss_bytes", lambda v: f"{v/1048576:.1f}"),
        ("Final RSS (MB)", "final_rss_bytes", lambda v: f"{v/1048576:.1f}"),
        ("Peak anon RSS (MB)", "peak_rss_anon_bytes", lambda v: f"{v/1048576:.1f}"),
        ("Peak PSS (MB)", "peak_pss_bytes", lambda v: f"{v/1048576:.1f}"),
        ("Peak threads", "peak_threads", lambda v: f"{v:.0f}"),
        ("Peak open FDs", "peak_fds", lambda v: f"{v:.0f}"),
        ("CPU user (ms)", "cpu_user_s", lambda v: f"{v*1000:.1f}"),
        ("CPU sys  (ms)", "cpu_sys_s", lambda v: f"{v*1000:.1f}"),
        ("CPU total (ms)", "cpu_total_s", lambda v: f"{v*1000:.1f}"),
        ("Avg CPU (%)", "cpu_pct", lambda v: f"{v:.0f}"),
        ("Minor faults", "minflt", lambda v: f"{v:.0f}"),
        ("Major faults", "majflt", lambda v: f"{v:.0f}"),
        ("Vol ctx switch", "vol_ctxt", lambda v: f"{v:.0f}"),
        ("Invol ctx switch", "nonvol_ctxt", lambda v: f"{v:.0f}"),
        ("Block read (KB)", "io_read_bytes", lambda v: f"{v/1024:.0f}"),
        ("Block write (KB)", "io_write_bytes", lambda v: f"{v/1024:.0f}"),
    ]
    for label, key, fmt in rows:
        med = median_field(runs, key)
        if med is None:
            continue
        print(f"  {label:<20} {fmt(med):>10}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    default_bin = os.environ.get("JCODE_BENCH_BIN") or shutil.which("jcode") or "./target/release/jcode"
    ap.add_argument("binary", nargs="?", default=default_bin)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--sample-interval-ms", type=float, default=2.0)
    ap.add_argument("--client-hold-s", type=float, default=2.5,
                    help="how long to keep the cold client alive while sampling")
    ap.add_argument("--json", type=str, default=None, help="write full results to a JSON file")
    ap.add_argument("--skip-client", action="store_true")
    ap.add_argument("--skip-server", action="store_true")
    args = ap.parse_args()

    binary = args.binary
    if not os.path.exists(binary):
        resolved = shutil.which(binary)
        if resolved:
            binary = resolved
        else:
            print(f"Binary not found: {binary}", file=sys.stderr)
            return 1
    binary = os.path.realpath(binary)

    size = os.path.getsize(binary)
    print("=" * 64)
    print("jcode spawn profile: time + resource utilization")
    print("=" * 64)
    print(f"Binary : {binary}")
    print(f"Size   : {size/1048576:.1f} MB")
    print(f"Runs   : {args.runs}   sample interval: {args.sample_interval_ms} ms")
    print(f"Host   : {os.cpu_count()} CPUs, isolated JCODE_HOME per run")

    interval = args.sample_interval_ms / 1000.0
    results: dict = {"binary": binary, "binary_size_bytes": size, "runs": args.runs}

    # warm the page cache / binary load
    subprocess.run([binary, "--version"], capture_output=True, check=False)

    if not args.skip_server:
        print(f"\n[1/2] Profiling server daemon spawn ({args.runs} runs)...")
        server_runs = []
        ready = []
        for i in range(args.runs):
            r = profile_server_spawn(binary, interval)
            if r:
                server_runs.append(r)
                if r.get("ready_ms"):
                    ready.append(r["ready_ms"])
            print(f"  run {i+1}: ready={r.get('ready_ms', 0):.1f}ms "
                  f"peak_rss={mb(r.get('peak_rss_bytes')).strip()}MB "
                  f"threads={r.get('peak_threads')} fds={r.get('peak_fds')}")
        results["server"] = server_runs
        if ready:
            print(f"\n  Server ready (socket connectable): "
                  f"min={min(ready):.1f} median={statistics.median(ready):.1f} "
                  f"max={max(ready):.1f} ms")
        print_resource_block("Server daemon resource footprint", server_runs)

    if not args.skip_client and _HAVE_PTY:
        print(f"\n[2/2] Profiling cold client spawn in PTY ({args.runs} runs)...")
        client_runs = []
        spawned_server_runs = []
        totals = []
        ready = []
        for i in range(args.runs):
            r = profile_client_spawn(binary, interval, args.client_hold_s)
            if not r:
                continue
            client_runs.append(r["client"])
            if r.get("server"):
                spawned_server_runs.append(r["server"])
            sp = r.get("startup_profile")
            if sp and sp.get("total_ms"):
                totals.append(sp["total_ms"])
            if r.get("server_ready_ms"):
                ready.append(r["server_ready_ms"])
            results.setdefault("client_full", []).append(r)
            print(f"  run {i+1}: startup_profile_total={sp['total_ms'] if sp else 0:.1f}ms "
                  f"client_peak_rss={mb(r['client'].get('peak_rss_bytes')).strip()}MB "
                  f"server_ready={r.get('server_ready_ms', 0):.1f}ms")
        if totals:
            print(f"\n  Startup-profile total (in-process marks): "
                  f"min={min(totals):.1f} median={statistics.median(totals):.1f} "
                  f"max={max(totals):.1f} ms")
        if ready:
            print(f"  Server ready from client launch: "
                  f"min={min(ready):.1f} median={statistics.median(ready):.1f} "
                  f"max={max(ready):.1f} ms")
        # phase breakdown
        if client_runs:
            full = results.get("client_full", [])
            phase_keys = [
                "args_parse", "selfdev_git_hash", "tui_client_enter",
                "tui_terminal_init", "server_spawn_start", "server_ready",
                "app_new_for_remote",
            ]
            print("\n  Startup phase breakdown (median ms):")
            for k in phase_keys:
                vals = [
                    f["startup_profile"]["deltas_ms"].get(k)
                    for f in full
                    if f.get("startup_profile")
                    and f["startup_profile"]["deltas_ms"].get(k) is not None
                ]
                if vals:
                    print(f"    {k:<22} {statistics.median(vals):7.2f}")
        print_resource_block("Cold client process resource footprint", client_runs)
        if spawned_server_runs:
            print_resource_block("Server daemon (spawned by client) footprint", spawned_server_runs)

    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=2))
        print(f"\nWrote full results to {args.json}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
