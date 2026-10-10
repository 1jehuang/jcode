#!/usr/bin/env python3
"""Heap-profile diff for idle session open/close churn.

Runs a jemalloc-prof jcode server with sampling on from start, does a warm-up
round of idle sessions, dumps heap A, runs N more rounds, dumps heap B, and
prints `jeprof --base=A B` top entries: allocations live after B that were not
live at A, i.e. memory retained per closed session.

Usage: scripts/heap_diff_idle_sessions.py --binary target/jeprof/selfdev/jcode
"""
from __future__ import annotations

import argparse
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from repro_idle_session_rss import mem, open_session, wait_for  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--sessions", type=int, default=30)
    ap.add_argument("--rounds", type=int, default=4)
    ap.add_argument("--settle", type=float, default=40.0)
    ap.add_argument("--abrupt", action="store_true")
    ap.add_argument("--realistic", action="store_true")
    args = ap.parse_args()
    binary = os.path.abspath(args.binary)

    import tempfile
    root = tempfile.mkdtemp(prefix="jcode-heapdiff-")
    home, run, work, prof = (Path(root) / d for d in ("home", "run", "work", "prof"))
    for d in (home, run, work, prof):
        d.mkdir()
    real = Path.home() / ".jcode"
    for name in ("auth.json", "anthropic-auth.json", "openai-auth.json", "config.toml", "models"):
        if (real / name).exists():
            (home / name).symlink_to(real / name)
    prefix = str(prof / "h")
    env = os.environ.copy()
    env.update({
        "JCODE_HOME": str(home), "JCODE_RUNTIME_DIR": str(run), "JCODE_TEMP_SERVER": "1",
        "JCODE_SERVER_OWNER_PID": str(os.getpid()), "JCODE_NO_TELEMETRY": "1",
        "JCODE_DEBUG_CONTROL": "1",
        # Sample every ~32KB (2^15) and accumulate nothing: live-heap dumps.
        "MALLOC_CONF": f"prof_active:true,lg_prof_sample:15,prof_prefix:{prefix}",
    })
    sock = str(run / "t.sock")
    server = subprocess.Popen([binary, "--no-update", "--no-selfdev", "serve", "--socket", sock],
                              cwd=work, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=open(Path(root) / "server.log", "w"), preexec_fn=os.setsid)
    gdb = ["gdb", "-p", str(server.pid), "-batch", "-ex"]

    def heap_dump(name: str) -> str:
        before = set(prof.glob("*.heap"))
        # NULL newp: jemalloc writes <prof_prefix>.<pid>.<seq>.m<seq>.heap.
        cmd = 'call (int)mallctl("prof.dump", 0, 0, 0, 0)'
        r = subprocess.run(["sudo", "-n"] + gdb + [cmd], capture_output=True, text=True)
        new = sorted(set(prof.glob("*.heap")) - before)
        if not new:
            print(r.stdout[-800:], r.stderr[-800:], file=sys.stderr)
            raise RuntimeError("heap dump failed")
        path = prof / f"{name}.heap"
        new[-1].rename(path)
        return str(path)

    try:
        if not wait_for(sock, 30):
            return 1

        def churn():
            conns = [open_session(sock, str(work), args.abrupt, args.realistic) for _ in range(args.sessions)]
            for c in conns:
                c.close()
            time.sleep(args.settle)

        churn()
        a = heap_dump("A")
        print("A", mem(server.pid), flush=True)
        for _ in range(args.rounds):
            churn()
        b = heap_dump("B")
        print("B", mem(server.pid), flush=True)
        out = subprocess.run(["jeprof", "--text", "--lines", f"--base={a}", binary, b],
                             capture_output=True, text=True)
        (Path(root) / "diff.txt").write_text(out.stdout + out.stderr)
        print("\n".join(out.stdout.splitlines()[:45]))
        out2 = subprocess.run(["jeprof", "--text", "--cum", f"--base={a}", binary, b],
                              capture_output=True, text=True)
        (Path(root) / "diff_cum.txt").write_text(out2.stdout)
    finally:
        os.killpg(os.getpgid(server.pid), signal.SIGTERM)
        print(f"artifacts: {root}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
