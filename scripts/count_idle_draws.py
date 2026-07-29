#!/usr/bin/env python3
"""
Count *real* `terminal.draw` calls on a freshly spawned idle screen.

Why this exists
---------------
The `partial_repaints` / `full_repaints` counters only increment when the client
believes it is drawing the decorative animation. In the state this script
targets, the animation is scheduled but has no recorded area, so neither counter
moves while the client still runs a full render every tick. Both counters read
zero, which looks like a quiet client and is exactly backwards.

Real draws are counted here from the per-draw sample timestamps in
`draw-stats`, which are recorded unconditionally by `record_draw_call_attribution`
and therefore cannot be fooled by the animation bookkeeping.
"""
from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from diagnose_idle_render_cost import (  # noqa: E402
    Client, client_cmd, dbg, launch, settle, wait_for_socket,
)


def snapshot(cmd_path: Path, resp_path: Path) -> tuple[dict, dict, dict, list]:
    ds = json.loads(client_cmd(cmd_path, resp_path, "draw-stats 240"))
    return (ds,
            ds.get("redraw_schedule") or {},
            ds.get("idle_animation") or {},
            ds.get("samples") or [])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--window-s", type=float, default=3.0)
    ap.add_argument("--type-slash", action="store_true",
                    help="open the slash palette before measuring")
    ap.add_argument("--no-idle-animation", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-drawcount-", dir=str(scratch)))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    env.update({
        "JCODE_HOME": str(home), "JCODE_RUNTIME_DIR": str(run),
        "JCODE_SOCKET": str(run / "jcode.sock"), "JCODE_NO_TELEMETRY": "1",
        "JCODE_DEBUG_CONTROL": "1", "JCODE_TEMP_SERVER": "1",
        "JCODE_SERVER_OWNER_PID": str(os.getpid()), "JCODE_PERF_TIER": "full",
        # See the note in the other harness scripts: never let the harness race
        # the client for the OSC 11 reply.
        "JCODE_THEME": "dark",
    })
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-drawcount")
    if args.no_idle_animation:
        env["JCODE_IDLE_ANIMATION"] = "false"
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client: Client | None = None
    out: dict = {"binary": binary}
    try:
        wait_for_socket(Path(env["JCODE_SOCKET"]))
        wait_for_socket(debug_sock)
        sid = dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        if sid.startswith("{"):
            sid = json.loads(sid).get("session_id", "")
        sid = sid.split()[-1] if sid else ""
        client = launch(binary, env, sid, cmd_path, resp_path)
        if not settle(cmd_path, resp_path):
            print("client never came up")
            return 3
        time.sleep(3.0)

        if args.type_slash:
            client.send(b"/")
            time.sleep(1.5)
            out["composer"] = client_cmd(cmd_path, resp_path, "input").strip()

        _, sched, anim, samples = snapshot(cmd_path, resp_path)
        out["donut_active"] = sched.get("idle_animation_active")
        out["animation_area"] = sched.get("idle_animation_area")
        out["interval_ms"] = sched.get("interval_ms")
        out["blocked_before"] = anim.get("fast_path_blocked")
        last_ts = samples[-1]["timestamp_ms"] if samples else None
        partial0 = anim.get("partial_repaints") or 0
        full0 = anim.get("full_repaints") or 0
        cpu0 = client.cpu_seconds()

        time.sleep(args.window_s)
        _, sched, anim, samples = snapshot(cmd_path, resp_path)
        cpu1 = client.cpu_seconds()

        recent = ([s for s in samples if s["timestamp_ms"] > last_ts]
                  if last_ts is not None else samples)
        out["real_draws"] = len(recent)
        out["real_draws_per_s"] = round(len(recent) / args.window_s, 1)
        if recent:
            rs = sorted(s["render_ms"] for s in recent)
            out["render_ms"] = {
                "p50": round(rs[len(rs) // 2], 2),
                "p95": round(rs[min(len(rs) - 1, int(len(rs) * 0.95))], 2),
                "max": round(rs[-1], 2),
                "sum": round(sum(rs), 1),
            }
            chg = [s["changed_cells"] for s in recent
                   if s.get("changed_cells") is not None]
            if chg:
                chg.sort()
                out["changed_cells"] = {
                    "p50": chg[len(chg) // 2], "max": chg[-1],
                    "total_cells": recent[-1].get("total_cells"),
                }
        out["partial_delta"] = (anim.get("partial_repaints") or 0) - partial0
        out["full_delta"] = (anim.get("full_repaints") or 0) - full0
        out["blocked_after"] = anim.get("fast_path_blocked")
        if cpu0 is not None and cpu1 is not None:
            out["cpu_cores"] = round((cpu1 - cpu0) / args.window_s, 3)
        # Fraction of wall time the client spent inside render, which is the
        # headroom a keystroke has to compete for.
        if out.get("render_ms"):
            out["render_duty_cycle"] = round(
                out["render_ms"]["sum"] / (args.window_s * 1000.0), 3)

        print(json.dumps(out, indent=2))
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
        import shutil
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
