#!/usr/bin/env python3
"""
Sweep the decorative animation frame rate on a real session and report the CPU
and keystroke-latency tradeoff.

Why this exists
---------------
The animation's cost scales with its frame rate, and 60fps was chosen without a
measurement of what it buys. On the user's real session the animation costs ~0.3
CPU cores at 60fps. This measures the curve so the default can be picked from
data instead of taste: CPU, keystroke latency, and how smooth the motion actually
is (frames actually delivered per second).

A decorative animation should cost a small fraction of a core and leave input
latency untouched. Terminal animation is generally indistinguishable above ~30fps
because each frame is a coarse glyph change, so the interesting question is where
the curve flattens.

Usage
-----
  python3 scripts/sweep_animation_fps.py [--binary PATH] [--fps 60 30 24 15]
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import repro_slash_flicker as flick  # noqa: E402
from repro_real_spawn_lag import recent_session  # noqa: E402


def cpu_seconds(pid: int) -> float | None:
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    except Exception:
        return None


def run(binary: str, session: str, fps: int | None, window_s: float,
        rows: int, cols: int) -> dict:
    flick.ROWS, flick.COLS = rows, cols
    runtime = Path(os.environ.get("JCODE_RUNTIME_DIR")
                   or f"/run/user/{os.getuid()}")
    env = os.environ.copy()
    env["JCODE_SOCKET"] = env.get("JCODE_SOCKET") or str(runtime / "jcode.sock")
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_THEME"] = "dark"
    if fps is None:
        env["JCODE_IDLE_ANIMATION"] = "false"
    else:
        env["JCODE_ANIMATION_FPS"] = str(fps)

    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-fps-", dir=str(scratch)))
    cmd_path, resp_path = root / "client_cmd", root / "client_resp"

    client = None
    try:
        client = flick.launch(binary, env, session, cmd_path, resp_path)
        if not flick.settle(cmd_path, resp_path, timeout_s=90.0):
            return {"fps": fps, "error": "client never came up"}
        # Let the launch burst (config parse, catalog, memory index) drain so it
        # is not attributed to the animation.
        time.sleep(9.0)

        def anim_counters() -> tuple[int, dict]:
            payload = json.loads(flick.client_cmd(cmd_path, resp_path,
                                                  "draw-stats 1", timeout_s=15.0))
            a = payload.get("idle_animation") or {}
            return (a.get("partial_repaints") or 0,
                    payload.get("redraw_schedule") or {})

        partial0, sched = anim_counters()
        cpu0 = cpu_seconds(client.proc.pid)
        t0 = time.monotonic()
        time.sleep(window_s)
        elapsed = time.monotonic() - t0
        partial1, _ = anim_counters()
        cpu1 = cpu_seconds(client.proc.pid)

        # Keystroke latency under exactly this animation load.
        lat: list[float] = []
        for ch in "the quick brown fox jumps":
            mark = len(client.output_events)
            k0 = time.monotonic()
            client.send(ch.encode())
            deadline = k0 + 2.0
            while time.monotonic() < deadline:
                if len(client.output_events) > mark:
                    lat.append((time.monotonic() - k0) * 1000.0)
                    break
                time.sleep(0.001)
            time.sleep(0.05)
        flick.client_cmd(cmd_path, resp_path, "set_input:")

        out = {
            "fps": fps,
            "interval_ms": sched.get("interval_ms"),
            "donut_active": sched.get("idle_animation_active"),
            "animation_frames_per_s": round((partial1 - partial0) / elapsed, 1),
            "cpu_cores": (round((cpu1 - cpu0) / elapsed, 3)
                          if cpu0 is not None and cpu1 is not None else None),
        }
        if lat:
            lat.sort()
            out["typing_p50_ms"] = round(statistics.median(lat), 2)
            out["typing_p95_ms"] = round(
                lat[min(len(lat) - 1, int(len(lat) * 0.95))], 2)
        return out
    finally:
        if client:
            client.shutdown()
        import shutil
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--session", default=None)
    ap.add_argument("--fps", type=int, nargs="*", default=[60, 30, 20, 12])
    ap.add_argument("--window-s", type=float, default=5.0)
    ap.add_argument("--rows", type=int, default=48)
    ap.add_argument("--cols", type=int, default=160)
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    runtime = Path(os.environ.get("JCODE_RUNTIME_DIR")
                   or f"/run/user/{os.getuid()}")
    session = args.session or recent_session(runtime / "jcode-debug.sock",
                                             str(REPO_ROOT))
    if not session:
        print("could not resolve a session")
        return 3

    print("== animation frame-rate sweep on a real session ==")
    print(f"  binary : {binary}")
    print(f"  session: {session}\n")
    print(f"  {'fps':>6} {'tick':>6} {'frames/s':>9} {'cpu':>7} "
          f"{'type p50':>9} {'type p95':>9}")

    rows = []
    for fps in [*args.fps, None]:
        r = run(binary, session, fps, args.window_s, args.rows, args.cols)
        rows.append(r)
        label = "off" if fps is None else str(fps)
        print(f"  {label:>6} {str(r.get('interval_ms')):>6} "
              f"{str(r.get('animation_frames_per_s')):>9} "
              f"{str(r.get('cpu_cores')):>7} "
              f"{str(r.get('typing_p50_ms')):>9} "
              f"{str(r.get('typing_p95_ms')):>9}")

    off = next((r for r in rows if r.get("fps") is None), None)
    if off and off.get("cpu_cores") is not None:
        print(f"\n  baseline (animation off): {off['cpu_cores']} cores")
        for r in rows:
            if r.get("fps") is None or r.get("cpu_cores") is None:
                continue
            print(f"    {r['fps']:>3}fps costs "
                  f"{round(r['cpu_cores'] - off['cpu_cores'], 3)} cores "
                  f"over baseline")
    return 0


if __name__ == "__main__":
    sys.exit(main())
