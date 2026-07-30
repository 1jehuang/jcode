#!/usr/bin/env python3
"""
Measure the CPU the decorative animation costs on a real session, with the
animation on and off, and check what that leaves for keystrokes.

Why this exists
---------------
On the user's real environment `draw-stats` reports `draws_per_s: 0.0` while the
client burns ~0.3 CPU cores. That is not a contradiction: the animation-only
partial repaint path deliberately skips `record_draw_call_attribution`, so its
~60 repaints per second are invisible to every draw counter. `perf` on that
client attributed the burn to `sample_orbit_rings`, `blit_idle`, and full-screen
`Cell::clone` / `to_vec`.

CPU is therefore the only honest signal here, and it must be measured as an A/B
against `JCODE_IDLE_ANIMATION=false`, otherwise "0.3 cores" cannot be attributed
to the animation rather than to ordinary client work.

Usage
-----
  python3 scripts/measure_animation_cpu_cost.py [--binary PATH]
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


def load_average() -> float:
    """1-minute load average, used to reject polluted measurements.

    Keystroke latency is measured in milliseconds, so a concurrent compile can
    dwarf the effect under test: a run taken at load 16 reported 9ms typing with
    the animation *inactive*, which says nothing about the animation. Recording
    the load makes such runs identifiable instead of silently misleading.
    """
    try:
        return float(Path("/proc/loadavg").read_text().split()[0])
    except Exception:
        return float("nan")


def cpu_seconds(pid: int) -> float | None:
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    except Exception:
        return None


def run_once(binary: str, session: str, animation: bool, window_s: float,
             rows: int, cols: int) -> dict:
    flick.ROWS, flick.COLS = rows, cols
    runtime = Path(os.environ.get("JCODE_RUNTIME_DIR")
                   or f"/run/user/{os.getuid()}")
    env = os.environ.copy()
    env["JCODE_SOCKET"] = env.get("JCODE_SOCKET") or str(runtime / "jcode.sock")
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_THEME"] = "dark"
    if not animation:
        env["JCODE_IDLE_ANIMATION"] = "false"

    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-animcpu-", dir=str(scratch)))
    cmd_path, resp_path = root / "client_cmd", root / "client_resp"

    client = None
    try:
        client = flick.launch(binary, env, session, cmd_path, resp_path)
        if not flick.settle(cmd_path, resp_path, timeout_s=90.0):
            return {"error": "client never came up", "animation": animation}
        # Let the launch burst (config parse, catalog, memory index) drain.
        # Those run once and would otherwise be attributed to the animation.
        time.sleep(10.0)

        sched = (json.loads(flick.client_cmd(cmd_path, resp_path, "draw-stats 1"))
                 .get("redraw_schedule") or {})

        cpu0 = cpu_seconds(client.proc.pid)
        t0 = time.monotonic()
        time.sleep(window_s)
        elapsed = time.monotonic() - t0
        cpu1 = cpu_seconds(client.proc.pid)

        anim = (json.loads(flick.client_cmd(cmd_path, resp_path, "draw-stats 1"))
                .get("idle_animation") or {})

        # Keystroke latency under exactly this load.
        lat: list[float] = []
        for ch in "the quick brown fox":
            mark = len(client.output_events)
            k0 = time.monotonic()
            client.send(ch.encode())
            deadline = k0 + 2.0
            while time.monotonic() < deadline:
                if len(client.output_events) > mark:
                    lat.append((time.monotonic() - k0) * 1000.0)
                    break
                time.sleep(0.001)
            time.sleep(0.06)
        flick.client_cmd(cmd_path, resp_path, "set_input:")

        out = {
            "animation": animation,
            "load_average": round(load_average(), 2),
            "donut_active": sched.get("idle_animation_active"),
            "interval_ms": sched.get("interval_ms"),
            "partial_repaints_total": anim.get("partial_repaints"),
            "cpu_cores": (round((cpu1 - cpu0) / elapsed, 3)
                          if cpu0 is not None and cpu1 is not None else None),
        }
        if lat:
            lat.sort()
            out["typing_p50_ms"] = round(statistics.median(lat), 2)
            out["typing_p95_ms"] = round(lat[min(len(lat) - 1,
                                                 int(len(lat) * 0.95))], 2)
            out["typing_max_ms"] = round(lat[-1], 2)
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
    ap.add_argument("--window-s", type=float, default=5.0)
    ap.add_argument("--repeat", type=int, default=2)
    ap.add_argument("--rows", type=int, default=48)
    ap.add_argument("--cols", type=int, default=160)
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    runtime = Path(os.environ.get("JCODE_RUNTIME_DIR")
                   or f"/run/user/{os.getuid()}")
    debug_sock = runtime / "jcode-debug.sock"
    session = args.session or recent_session(debug_sock, str(REPO_ROOT))
    if not session:
        session = flick.dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        session = session.split()[-1] if session else ""
    if not session:
        print("could not resolve a session")
        return 3

    print("== animation CPU cost on a real session ==")
    print(f"  binary : {binary}")
    print(f"  session: {session}\n")

    rows = []
    for _ in range(max(1, args.repeat)):
        for animation in (True, False):
            r = run_once(binary, session, animation, args.window_s,
                         args.rows, args.cols)
            rows.append(r)
            label = "donut ON " if animation else "donut OFF"
            warn = ""
            load = r.get("load_average")
            if load is not None and load == load and load > 4.0:
                # A busy machine makes millisecond latency numbers meaningless.
                warn = f"  [!] load={load}, latency unreliable"
            print(f"  {label}: cpu={r.get('cpu_cores')} cores  "
                  f"interval={r.get('interval_ms')}ms  "
                  f"typing p50={r.get('typing_p50_ms')}ms "
                  f"p95={r.get('typing_p95_ms')}ms{warn}")

    on = [r["cpu_cores"] for r in rows
          if r.get("animation") and r.get("cpu_cores") is not None]
    off = [r["cpu_cores"] for r in rows
           if not r.get("animation") and r.get("cpu_cores") is not None]
    if on and off:
        on_m, off_m = statistics.median(on), statistics.median(off)
        print(f"\n  median CPU: donut ON {on_m} cores, OFF {off_m} cores")
        print(f"  attributable to the animation: {round(on_m - off_m, 3)} cores")
        if on_m - off_m > 0.05:
            print("  -> the decorative animation is the dominant idle cost")
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
