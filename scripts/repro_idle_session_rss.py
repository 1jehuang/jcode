#!/usr/bin/env python3
"""Measure server RSS growth from opening and closing idle sessions.

Starts an isolated `jcode serve`, then repeatedly opens N client connections
that only Subscribe (no messages), closes them, waits past the idle reconnect
grace, and samples server RSS/anon. RSS that keeps climbing round over round
after sessions are gone indicates retained per-session state.

Usage: scripts/repro_idle_session_rss.py [--binary PATH] [--sessions N] [--rounds R]
"""
from __future__ import annotations

import argparse
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def mem(pid: int) -> dict[str, int]:
    out = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        k, _, v = line.partition(":")
        if k in ("VmRSS", "RssAnon", "VmHWM", "Threads"):
            out[k] = int(v.split()[0])
    return out


def wait_for(path: str, timeout: float) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        if os.path.exists(path):
            return True
        time.sleep(0.1)
    return False


def open_session(sock_path: str, cwd: str, abrupt: bool = False, realistic: bool = False) -> socket.socket:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock_path)
    req = {"type": "subscribe", "id": 1, "working_dir": cwd}
    s.sendall((json.dumps(req) + "\n").encode())
    if realistic:
        # Follow-up requests a TUI sends right after attaching.
        for i, kind in enumerate(("get_history", "get_model_catalog", "state", "ping"), start=2):
            s.sendall((json.dumps({"type": kind, "id": i}) + "\n").encode())
    if abrupt:
        # Close while subscribe is still in flight, like a window closed
        # immediately after spawning.
        return s
    s.settimeout(15)
    buf = b""
    end = time.time() + 15
    # Wait for the subscribe to be acknowledged (Done for id 1) or history.
    while time.time() < end:
        try:
            chunk = s.recv(1 << 16)
        except socket.timeout:
            break
        if not chunk:
            break
        buf += chunk
        if b'"type":"done"' in buf or b'"type":"history"' in buf:
            if realistic:
                time.sleep(0.5)
            break
    return s


def debug(sock_path: str, cmd: str) -> str:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(30)
    s.connect(sock_path)
    s.sendall((json.dumps({"type": "debug_command", "id": 1, "command": cmd}) + "\n").encode())
    data = b""
    while b"\n" not in data:
        chunk = s.recv(1 << 20)
        if not chunk:
            break
        data += chunk
    s.close()
    try:
        return json.loads(data.split(b"\n", 1)[0]).get("output", "")
    except Exception:
        return data.decode(errors="replace")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default=os.path.realpath(os.path.expanduser("~/.jcode/builds/shared-server/jcode")))
    ap.add_argument("--sessions", type=int, default=20)
    ap.add_argument("--rounds", type=int, default=4)
    ap.add_argument("--abrupt", action="store_true", help="close before subscribe completes")
    ap.add_argument("--settle", type=float, default=40.0, help="seconds to wait after close (grace is 30s)")
    args = ap.parse_args()

    root = tempfile.mkdtemp(prefix="jcode-idle-rss-")
    home = Path(root) / "home"
    run = Path(root) / "run"
    work = Path(root) / "work"
    for d in (home, run, work):
        d.mkdir()
    real = Path.home() / ".jcode"
    for name in ("auth.json", "anthropic-auth.json", "openai-auth.json", "config.toml", "models"):
        if (real / name).exists():
            (home / name).symlink_to(real / name)
    env = os.environ.copy()
    env.update({
        "JCODE_HOME": str(home),
        "JCODE_RUNTIME_DIR": str(run),
        "JCODE_TEMP_SERVER": "1",
        "JCODE_SERVER_OWNER_PID": str(os.getpid()),
        "JCODE_NO_TELEMETRY": "1",
        "JCODE_DEBUG_CONTROL": "1",
    })
    sock = str(run / "t.sock")
    dbg = str(run / "t-debug.sock")
    server = subprocess.Popen(
        [args.binary, "--no-update", "--no-selfdev", "serve", "--socket", sock],
        cwd=work, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=open(Path(root) / "server.log", "w"), preexec_fn=os.setsid,
    )
    try:
        if not wait_for(sock, 30):
            print("server socket never appeared", file=sys.stderr)
            return 1
        # Warm-up: one session open/close so one-time init is excluded.
        open_session(sock, str(work)).close()
        time.sleep(args.settle)
        base = mem(server.pid)
        print(json.dumps({"phase": "baseline", **base}), flush=True)
        for r in range(args.rounds):
            conns = [open_session(sock, str(work), args.abrupt) for _ in range(args.sessions)]
            opened = mem(server.pid)
            for c in conns:
                c.close()
            time.sleep(args.settle)
            after = mem(server.pid)
            try:
                info = json.loads(debug(dbg, "info"))
                live = info.get("session_count", -1)
                members = info.get("swarm_member_count", -1)
            except Exception:
                live = members = -1
            alloc = {}
            try:
                st = json.loads(debug(dbg, "server:memory"))["process"]["allocator"]["stats"]
                alloc = {"heap_allocated_kb": (st.get("allocated_bytes") or 0) // 1024,
                         "heap_mapped_kb": (st.get("mapped_bytes") or 0) // 1024,
                         "heap_retained_kb": (st.get("retained_bytes") or 0) // 1024}
            except Exception:
                pass
            print(json.dumps({
                "phase": f"round{r + 1}", **alloc,
                "open_rss": opened["VmRSS"], "after_rss": after["VmRSS"],
                "after_anon": after["RssAnon"], "threads": after["Threads"],
                "delta_anon_vs_base": after["RssAnon"] - base["RssAnon"],
                "live_sessions": live, "swarm_members": members,
            }), flush=True)
        if os.path.exists(dbg):
            # Distinguish retained-but-free allocator pages from live growth.
            subprocess.run(["sudo", "-n", "gdb", "-p", str(server.pid), "-batch", "-ex",
                            "call (int)malloc_trim(0)"], capture_output=True)
            trimmed = mem(server.pid)
            print(json.dumps({"phase": "after_trim", "rss": trimmed["VmRSS"],
                              "anon": trimmed["RssAnon"],
                              "delta_anon_vs_base": trimmed["RssAnon"] - base["RssAnon"]}), flush=True)
            out = Path(root) / "server_memory.json"
            out.write_text(debug(dbg, "server:memory"))
            print(f"server:memory -> {out}")
    finally:
        os.killpg(os.getpgid(server.pid), signal.SIGTERM)
        print(f"logs: {root}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
