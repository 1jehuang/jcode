#!/usr/bin/env python3
"""
Profile a live, freshly spawned jcode TUI while the decorative idle animation
is running, and report where its CPU actually goes.

Why this exists
---------------
`draw-stats` says a frame took 3.3s and changed 0 cells, which proves the cost
is inside `ui::draw` but not *where*. Reading the code cannot settle it either:
the draw path fans out into transcript preparation, chrome, info widgets, and
the animation samplers. This attaches `perf` to the real client process in the
exact state users complain about, so the answer comes from measurement.

Usage
-----
  sudo -n true && python3 scripts/profile_idle_donut.py [--binary PATH]

`perf record` usually needs `kernel.perf_event_paranoid <= 1`. The script
reports clearly if it cannot sample instead of silently producing nothing.
"""
from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import fcntl
import struct
import termios
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from diagnose_idle_render_cost import (  # noqa: E402
    Client, client_cmd, dbg, launch, settle, wait_for_socket,
)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    default_bin = REPO_ROOT / "target" / "selfdev" / "jcode"
    ap.add_argument("--binary", default=str(default_bin))
    ap.add_argument("--seconds", type=float, default=8.0)
    ap.add_argument("--freq", type=int, default=997)
    ap.add_argument("--palette", action="store_true",
                    help="profile with the slash palette open")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"binary not found: {binary}")
        return 3

    root = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-profile-donut-", dir=str(root)))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    env["JCODE_HOME"] = str(home)
    env["JCODE_RUNTIME_DIR"] = str(run)
    env["JCODE_SOCKET"] = str(run / "jcode.sock")
    env["JCODE_NO_TELEMETRY"] = "1"
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_TEMP_SERVER"] = "1"
    env["JCODE_SERVER_OWNER_PID"] = str(os.getpid())
    env["JCODE_PERF_TIER"] = "full"
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-profile-donut")
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    print(f"== profiling idle donut ==\n  binary: {binary}")
    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client: Client | None = None
    try:
        wait_for_socket(Path(env["JCODE_SOCKET"]))
        wait_for_socket(debug_sock)
        session_id = dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        if session_id.startswith("{"):
            session_id = json.loads(session_id).get("session_id", "")
        session_id = session_id.split()[-1] if session_id else ""
        client = launch(binary, env, session_id, cmd_path, resp_path)
        if not settle(cmd_path, resp_path):
            print("client never came up")
            return 3
        time.sleep(3.0)

        sched = (json.loads(client_cmd(cmd_path, resp_path, "draw-stats 1"))
                 .get("redraw_schedule") or {})
        print(f"  donut active: {sched.get('idle_animation_active')} "
              f"interval={sched.get('interval_ms')}ms "
              f"tier={sched.get('tier')}")
        if not sched.get("idle_animation_active"):
            print("  WARNING: the donut is not running, so this profile will "
                  "not represent the reported state.")

        if args.palette:
            client.send(b"/")
            time.sleep(1.0)

        data = root / "perf.data"
        print(f"  sampling {args.seconds}s at {args.freq}Hz ...")
        rec = subprocess.run(
            ["perf", "record", "-F", str(args.freq), "-g",
             "--pid", str(client.proc.pid), "-o", str(data),
             "--", "sleep", str(args.seconds)],
            capture_output=True, text=True)
        if rec.returncode != 0 or not data.exists():
            print("  perf record failed:")
            print((rec.stderr or rec.stdout)[-2000:])
            print("  hint: sysctl -w kernel.perf_event_paranoid=1")
            return 3

        rep = subprocess.run(
            ["perf", "report", "-i", str(data), "--stdio", "--no-children",
             "--percent-limit", "0.7", "-g", "none"],
            capture_output=True, text=True)
        print("\n=== self time (flat) ===")
        print("\n".join(l for l in rep.stdout.splitlines()
                        if l.strip() and not l.startswith("#"))[:4000])

        rep2 = subprocess.run(
            ["perf", "report", "-i", str(data), "--stdio", "--children",
             "--percent-limit", "2.0", "-g", "none"],
            capture_output=True, text=True)
        print("\n=== inclusive time (children) ===")
        print("\n".join(l for l in rep2.stdout.splitlines()
                        if l.strip() and not l.startswith("#"))[:4000])
        return 0
    finally:
        if client:
            client.shutdown()
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(server.pid, sig)
                server.wait(timeout=3.0)
                break
            except (ProcessLookupError, PermissionError):
                break
            except Exception:
                continue
        print(f"\n  (artifacts in {root})")


if __name__ == "__main__":
    sys.exit(main())
