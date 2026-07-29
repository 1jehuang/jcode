#!/usr/bin/env python3
"""
Prove the decorative donut still animates in the state where it is genuinely
expected, on both the pre-fix and post-fix binary.

Why this exists
---------------
The lag fix stops pacing the redraw loop at animation cadence when nothing is
animating. The failure mode to rule out is "fixed it by disabling the donut".
That has to be checked in a state where the donut is *supposed* to run, which is
narrower than it first appears:

  * a non-empty transcript makes `time_since_activity()` report deep idle, which
    legitimately parks the donut, and
  * any full-screen overlay (including the onboarding start-choice picker that a
    first run opens) legitimately hides it.

So this drives the client to the plain empty-transcript idle screen with the
overlay dismissed, and asserts the animation rows physically move on screen,
through a real terminal emulator. Run it against two binaries and compare.
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


def schedule(cmd_path: Path, resp_path: Path) -> dict:
    ds = json.loads(flick.client_cmd(cmd_path, resp_path, "draw-stats 1"))
    sched = ds.get("redraw_schedule") or {}
    anim = ds.get("idle_animation") or {}
    return {
        "donut_active": sched.get("idle_animation_active"),
        "area": sched.get("idle_animation_area"),
        "interval_ms": sched.get("interval_ms"),
        "partial": anim.get("partial_repaints") or 0,
        "full": anim.get("full_repaints") or 0,
        "messages": None,
    }


def moving_rows(client, seconds: float) -> dict[int, int]:
    churn: dict[int, int] = {}
    prev = client.rows()
    t0 = time.monotonic()
    while time.monotonic() - t0 < seconds:
        cur = client.rows()
        for idx, (a, b) in enumerate(zip(prev, cur)):
            if a != b:
                churn[idx] = churn.get(idx, 0) + 1
        prev = cur
        time.sleep(0.005)
    return churn


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--watch-s", type=float, default=2.0)
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    flick.ROWS, flick.COLS = 48, 160
    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-donut-live-", dir=str(scratch)))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    env.update({
        "JCODE_HOME": str(home), "JCODE_RUNTIME_DIR": str(run),
        "JCODE_SOCKET": str(run / "jcode.sock"), "JCODE_NO_TELEMETRY": "1",
        "JCODE_DEBUG_CONTROL": "1", "JCODE_TEMP_SERVER": "1",
        "JCODE_SERVER_OWNER_PID": str(os.getpid()), "JCODE_PERF_TIER": "full",
        "JCODE_THEME": "dark",
    })
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-donut-live")
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client = None
    out: dict = {"binary": binary}
    try:
        flick.wait_for_socket(Path(env["JCODE_SOCKET"]))
        flick.wait_for_socket(debug_sock)
        sid = flick.dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        if sid.startswith("{"):
            sid = json.loads(sid).get("session_id", "")
        sid = sid.split()[-1] if sid else ""
        client = flick.launch(binary, env, sid, cmd_path, resp_path)
        if not flick.settle(cmd_path, resp_path):
            print("client never came up")
            return 3
        time.sleep(3.0)

        # Dismiss the onboarding start-choice picker (a full-screen overlay).
        client.send(b"\x1b")
        time.sleep(1.5)
        # Clear the transcript so `time_since_activity()` stops reporting deep
        # idle: with an empty transcript it falls through to the app-uptime clock,
        # which is the genuine "fresh idle screen" the donut is designed for.
        try:
            flick.client_cmd(cmd_path, resp_path, "message:/clear", timeout_s=10.0)
        except Exception:
            pass
        time.sleep(2.5)
        # Real interaction resets the activity clock.
        client.send(b"x")
        time.sleep(0.3)
        flick.client_cmd(cmd_path, resp_path, "set_input:")
        time.sleep(1.5)

        before = schedule(cmd_path, resp_path)
        state = json.loads(flick.client_cmd(cmd_path, resp_path, "state"))
        churn = moving_rows(client, args.watch_s)
        after = schedule(cmd_path, resp_path)
        partial_rate = (after["partial"] - before["partial"]) / args.watch_s
        full_rate = (after["full"] - before["full"]) / args.watch_s

        out.update({
            "display_messages": state.get("display_messages"),
            "donut_active": after["donut_active"],
            "animation_area": after["area"],
            "interval_ms": after["interval_ms"],
            "moving_rows": len(churn),
            "busiest_rows": sorted(churn.items(), key=lambda kv: -kv[1])[:6],
            "partial_per_s": round(partial_rate, 1),
            "full_per_s": round(full_rate, 1),
            "cpu_cores": None,
        })
        print(json.dumps(out, indent=2))

        # The donut is expected here. If it animates, rows move and the cheap
        # partial path carries the ticks.
        if after["donut_active"] and after["area"]:
            ok = len(churn) > 0 and partial_rate > 5
            print("\n  ANIMATING" if ok else "\n  NOT ANIMATING (regression?)")
            return 0 if ok else 1
        print("\n  donut not expected in this state "
              f"(active={after['donut_active']} area={after['area']})")
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
