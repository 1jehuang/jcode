#!/usr/bin/env python3
"""
Test whether early-session input lag ends when the model catalog resolves.

The hypothesis
--------------
A freshly spawned jcode feels laggy for the first few seconds and then becomes
responsive. The reported cause is the model catalog: while it is still being
fetched the client is in its remote-startup phase, and that phase pins the redraw
loop to a 1000ms cadence (`REDRAW_REMOTE_STARTUP`). If that is the mechanism then
keystroke latency should be high while the startup phase is active and drop
sharply at the moment it clears.

Why the earlier harnesses could not see this
--------------------------------------------
`repro_input_flicker.py` and `repro_input_lag.py` both type immediately after
spawn and finish within a couple of seconds. That is entirely inside the slow
window, so they measured startup and reported it as steady state: every keystroke
looked slow and nothing ever looked fast, which gives no signal about *when* the
lag ends.

What this does
--------------
Types one character every `--interval` seconds for `--duration` seconds, and for
each keystroke records both the paint latency (read from an emulated screen) and
whether the client still reports an active startup phase. That produces a latency
timeline that can be split at the startup->ready transition, so the hypothesis is
either confirmed (a clear drop at the boundary) or refuted (latency unchanged
across it).

Usage
-----
  python3 scripts/repro_startup_lag.py --binary PATH [--duration 25]

Exit codes:
  0 = latency does not depend on the startup phase (hypothesis refuted)
  1 = latency drops once startup/catalog resolves (hypothesis confirmed)
  3 = setup failure or not enough samples on one side of the boundary
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

# Reuse the emulated-screen client from the flicker harness: it already solves
# locating the composer and timing paints off the reader thread.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import repro_input_flicker as flicker  # noqa: E402


def startup_snapshot(cmd_path: Path, resp_path: Path) -> dict:
    """Ask the client whether it is still in its remote-startup phase."""
    try:
        raw = flicker.client_cmd(cmd_path, resp_path, "draw-stats 4", timeout_s=6.0)
        sched = json.loads(raw).get("redraw_schedule") or {}
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)}
    return {
        "interval_ms": sched.get("interval_ms"),
        "startup_active": sched.get("remote_startup_phase_active"),
        "catalog_models": sched.get("model_catalog_models"),
        "reason": sched.get("current_full_frame_reason"),
    }


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--binary", required=True)
    ap.add_argument("--duration", type=float, default=25.0,
                    help="how long to keep typing, in seconds. Must comfortably "
                         "outlast catalog resolution or there is nothing to "
                         "compare the slow window against.")
    ap.add_argument("--interval", type=float, default=0.9,
                    help="seconds between keystrokes")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"binary not found: {binary}")
        return 3

    root = Path(tempfile.mkdtemp(prefix="jcode-startup-lag-"))
    run = root / "run"
    run.mkdir(parents=True)

    env = os.environ.copy()
    # Live server and real home: catalog resolution is the thing under test, so a
    # throwaway home with no providers would skip it entirely.
    runtime = Path(env.get("JCODE_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
    env["JCODE_SOCKET"] = env.get("JCODE_SOCKET") or str(runtime / "jcode.sock")
    env["JCODE_DEBUG_CONTROL"] = "1"
    debug_sock = runtime / "jcode-debug.sock"
    cmd_path, resp_path = run / "cmd", run / "resp"

    if not args.json:
        print("== startup-lag vs model catalog ==")
        print(f"  binary  : {binary}")
        print(f"  duration: {args.duration}s @ {args.interval}s/keystroke")

    session_id = flicker.create_session(debug_sock, Path.cwd())
    client = flicker.launch_client(binary, env, session_id, cmd_path, resp_path)
    samples: list[dict] = []
    try:
        spawn_t0 = time.monotonic()
        if not flicker.settle(cmd_path, resp_path, timeout_s=60.0):
            print("client never came up on the debug channel")
            return 3
        if not args.json:
            print(f"  client  : up after "
                  f"{(time.monotonic() - spawn_t0) * 1000:.0f}ms\n")

        # Type a character, note its paint latency, and note whether startup is
        # still active. Text accumulates, which is fine: we only need each new
        # character to show up.
        typed = ""
        alphabet = "abcdefghijklmnopqrstuvwxyz"
        i = 0
        while time.monotonic() - spawn_t0 < args.duration:
            ch = alphabet[i % len(alphabet)]
            i += 1
            typed += ch
            state = startup_snapshot(cmd_path, resp_path)
            obs = flicker.observe_keystroke(client, ch, typed, watch_s=args.interval)
            since_spawn_ms = (time.monotonic() - spawn_t0) * 1000.0
            samples.append({
                "since_spawn_ms": round(since_spawn_ms, 1),
                "paint_ms": obs.first_seen_ms,
                "startup_active": state.get("startup_active"),
                "interval_ms": state.get("interval_ms"),
                "reason": state.get("reason"),
            })
            if not args.json:
                print(f"  t={since_spawn_ms / 1000:5.1f}s "
                      f"paint={obs.first_seen_ms}ms "
                      f"startup={state.get('startup_active')} "
                      f"tick={state.get('interval_ms')}ms")
            # Keep the composer from growing without bound (and from opening a
            # command menu that changes the layout under us).
            if len(typed) > 20:
                flicker.client_cmd(cmd_path, resp_path, "set_input:", timeout_s=5.0)
                typed = ""
    finally:
        client.shutdown()

    painted = [s for s in samples if s["paint_ms"] is not None]
    during = [s["paint_ms"] for s in painted if s["startup_active"] is True]
    after = [s["paint_ms"] for s in painted if s["startup_active"] is False]

    result = {
        "samples": samples,
        "during_startup": {
            "count": len(during),
            "p50_ms": round(statistics.median(during), 1) if during else None,
        },
        "after_startup": {
            "count": len(after),
            "p50_ms": round(statistics.median(after), 1) if after else None,
        },
    }

    if args.json:
        print(json.dumps(result, indent=2))

    if not during or not after:
        print(f"\n  inconclusive: {len(during)} samples during startup, "
              f"{len(after)} after. Cannot compare across the boundary.")
        print("  (if startup_active is always None, the binary predates the "
              "field being exposed in draw-stats)")
        return 3

    d, a = result["during_startup"]["p50_ms"], result["after_startup"]["p50_ms"]
    confirmed = d > a * 1.5
    print(f"\n  during startup: p50={d}ms ({len(during)} samples)")
    print(f"  after  startup: p50={a}ms ({len(after)} samples)")
    if confirmed:
        print("  => CONFIRMED: keystrokes are markedly slower until the catalog "
              "resolves.")
    else:
        print("  => REFUTED: latency does not drop when the catalog resolves, so "
              "the slow window has another cause.")
    return 1 if confirmed else 0


if __name__ == "__main__":
    raise SystemExit(main())
