#!/usr/bin/env python3
"""
Reproduce fresh-spawn lag against the *user's real environment*, not a throwaway
home.

Why this exists
---------------
`repro_input_lag.py` (isolated mode) creates an empty `JCODE_HOME`, so a fresh
client lands on the onboarding welcome screen with no transcript. That is not what
the user spawns into. Their real environment has:

  * a real config (`animation_fps = 60`, `redraw_fps = 60`, `idle_animation = true`),
  * a large session history, and
  * a real transcript in the resumed session.

The real client's own `draw-stats` shows the difference plainly: ~11ms render p50
to change ~190 of 2397 cells, and the daily log holds 2497 slow (>40ms) *tick*
frames, up to 249 in a single minute. So the expensive thing is the per-frame
render cost of a real screen, which an empty transcript never exercises.

This resumes an actual recent session, measures keystroke latency and the render
duty cycle (fraction of wall time spent inside render), and attributes cost.
A duty cycle near 1.0 means the render loop has no headroom left for input, which
is what "spawning a new one still lags" feels like.

Usage
-----
  python3 scripts/repro_real_spawn_lag.py [--binary PATH] [--session ID]
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

import repro_slash_flicker as flick  # noqa: E402


def recent_session(debug_sock: Path, working_dir: str) -> str | None:
    """Pick the most recent real session for this working dir."""
    try:
        raw = flick.dbg(debug_sock, "sessions", timeout=30.0)
        payload = json.loads(raw)
    except Exception:
        return None
    rows = payload if isinstance(payload, list) else payload.get("sessions") or []
    best = None
    for row in rows:
        if not isinstance(row, dict):
            continue
        if row.get("working_dir") not in (working_dir, None):
            continue
        # Prefer sessions with real content, since an empty one cannot reproduce
        # transcript render cost.
        msgs = row.get("message_count") or row.get("messages") or 0
        updated = row.get("updated_at") or row.get("last_active") or ""
        key = (msgs > 0, str(updated))
        if best is None or key > best[0]:
            best = (key, row.get("id") or row.get("session_id"))
    return best[1] if best else None


def measure(client, cmd_path: Path, resp_path: Path, label: str,
            window_s: float) -> dict:
    """Render duty cycle and draw cost over a window."""

    def stats() -> tuple[list, dict]:
        payload = json.loads(flick.client_cmd(cmd_path, resp_path,
                                              "draw-stats 240", timeout_s=15.0))
        return payload.get("samples") or [], payload.get("redraw_schedule") or {}

    samples, sched = stats()
    last_ts = samples[-1]["timestamp_ms"] if samples else None
    cpu0 = None
    try:
        fields = Path(f"/proc/{client.proc.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        cpu0 = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    except Exception:
        pass

    t0 = time.monotonic()
    time.sleep(window_s)
    elapsed = time.monotonic() - t0
    samples, sched = stats()
    cpu1 = None
    try:
        fields = Path(f"/proc/{client.proc.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        cpu1 = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    except Exception:
        pass

    fresh = ([s for s in samples if s["timestamp_ms"] > last_ts]
             if last_ts is not None else samples)
    out = {
        "state": label,
        "draws": len(fresh),
        "draws_per_s": round(len(fresh) / elapsed, 1),
        "interval_ms": sched.get("interval_ms"),
        "donut_active": sched.get("idle_animation_active"),
        "animation_area": sched.get("idle_animation_area"),
    }
    if fresh:
        rs = sorted(s["render_ms"] for s in fresh)
        out["render_ms"] = {
            "p50": round(rs[len(rs) // 2], 2),
            "p95": round(rs[min(len(rs) - 1, int(len(rs) * 0.95))], 2),
            "max": round(rs[-1], 2),
        }
        # The number that matters: how much of the wall clock is spent rendering.
        # Anything approaching 1.0 leaves no headroom for a keystroke.
        out["render_duty_cycle"] = round(sum(rs) / (elapsed * 1000.0), 3)
        chg = [s["changed_cells"] for s in fresh if s.get("changed_cells") is not None]
        if chg:
            chg.sort()
            out["changed_cells_p50"] = chg[len(chg) // 2]
            out["total_cells"] = fresh[-1].get("total_cells")
    if cpu0 is not None and cpu1 is not None:
        out["cpu_cores"] = round((cpu1 - cpu0) / elapsed, 3)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--session", default=None,
                    help="session id to resume (default: most recent real one)")
    ap.add_argument("--window-s", type=float, default=4.0)
    ap.add_argument("--rows", type=int, default=48)
    ap.add_argument("--cols", type=int, default=160)
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    flick.ROWS, flick.COLS = args.rows, args.cols
    binary = str(Path(args.binary).resolve())

    # Talk to the user's real server, with their real home/config/sessions. This
    # is the configuration the lag report came from.
    runtime = Path(os.environ.get("JCODE_RUNTIME_DIR")
                   or f"/run/user/{os.getuid()}")
    env = os.environ.copy()
    env["JCODE_SOCKET"] = env.get("JCODE_SOCKET") or str(runtime / "jcode.sock")
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_THEME"] = "dark"  # never race the client for the OSC 11 reply
    debug_sock = runtime / "jcode-debug.sock"

    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    run = Path(tempfile.mkdtemp(prefix="jcode-realspawn-", dir=str(scratch)))
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    print("== real-environment spawn lag ==")
    print(f"  binary : {binary}")
    print(f"  socket : {env['JCODE_SOCKET']}")

    session = args.session or recent_session(debug_sock, str(REPO_ROOT))
    if not session:
        session = flick.dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        session = session.split()[-1] if session else ""
    if not session:
        print("could not resolve a session to resume")
        return 3
    print(f"  session: {session}")

    client = None
    try:
        client = flick.launch(binary, env, session, cmd_path, resp_path)
        spawn_t0 = time.monotonic()
        if not flick.settle(cmd_path, resp_path, timeout_s=90.0):
            print("client never came up on the debug channel")
            return 3
        print(f"  client : up after {(time.monotonic() - spawn_t0) * 1000:.0f}ms\n")

        results = [measure(client, cmd_path, resp_path, "just-spawned",
                           args.window_s)]
        time.sleep(2.0)
        results.append(measure(client, cmd_path, resp_path, "settled",
                               args.window_s))

        # Typing latency through the real PTY, which is what the user feels.
        lat = []
        for ch in "hello world":
            mark = len(client.output_events)
            t0 = time.monotonic()
            client.send(ch.encode())
            deadline = t0 + 2.0
            while time.monotonic() < deadline:
                if len(client.output_events) > mark:
                    lat.append((time.monotonic() - t0) * 1000.0)
                    break
                time.sleep(0.001)
            time.sleep(0.12)
        flick.client_cmd(cmd_path, resp_path, "set_input:")

        if lat:
            lat.sort()
            typing = {
                "p50_ms": round(lat[len(lat) // 2], 2),
                "p95_ms": round(lat[min(len(lat) - 1, int(len(lat) * 0.95))], 2),
                "max_ms": round(lat[-1], 2),
            }
        else:
            typing = {"error": "no repaints observed"}

        payload = {"binary": binary, "session": session,
                   "states": results, "typing": typing}
        if args.json:
            print(json.dumps(payload, indent=2))
        else:
            for r in results:
                print(f"  [{r['state']}]")
                for key in ("draws_per_s", "interval_ms", "donut_active",
                            "render_ms", "render_duty_cycle",
                            "changed_cells_p50", "total_cells", "cpu_cores"):
                    if key in r:
                        print(f"    {key:20}: {r[key]}")
                print()
            print(f"  typing latency: {typing}")

        worst = max((r.get("render_duty_cycle") or 0) for r in results)
        if worst > 0.30:
            print(f"\n  LAG REPRODUCED: render duty cycle {worst} "
                  f"(>0.30 leaves little headroom for input)")
            return 1
        print(f"\n  render duty cycle {worst}: the loop has headroom")
        return 0
    finally:
        if client:
            client.shutdown()
        import shutil
        shutil.rmtree(run, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
