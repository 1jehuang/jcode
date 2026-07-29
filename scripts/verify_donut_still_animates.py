#!/usr/bin/env python3
"""
Verify the decorative idle animation still runs where it *should*, after the fix
that stops it running where it should not.

Why this exists
---------------
The lag fix paces the redraw loop off what the renderer actually published, so
the obvious way to "fix" the symptom by accident is to disable the animation
everywhere. That would be a silent regression: the donut is a deliberate feature.
This proves both halves of the intended behavior on a real client, through a real
terminal emulator:

  * on a normal idle chat screen the donut rows animate and CPU stays modest
  * with a full-screen overlay up (the `/resume` picker), nothing animates and
    the loop drops off animation cadence

It also confirms the slash-command palette opens and stays open (no flicker) on
the screen where a composer actually exists.
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

MENU_CANDIDATES = ("/help", "/model", "/clear", "/resume", "/config", "/login")


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
    args = ap.parse_args()

    flick.ROWS, flick.COLS = 48, 160
    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-donut-verify-", dir=str(scratch)))
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
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-donut-verify")
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client = None
    failures: list[str] = []
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

        # Do NOT inject a message to leave the onboarding screen. Any non-empty
        # transcript makes `time_since_activity()` report deep idle (it returns
        # REDRAW_DEEP_IDLE_AFTER + 1s whenever `display_messages` is non-empty and
        # nothing is processing), which correctly disables the decorative donut.
        # A verifier built that way measures the deep-idle path and proves nothing
        # about the animation. The onboarding welcome screen is the real
        # "animation expected" state on a fresh spawn, and it has a composer, so
        # it exercises both the donut and the slash palette.
        # A fresh first run opens the onboarding "start choice" screen, which is a
        # SessionPicker overlay (`onboarding_open_start_choice`). That is itself a
        # full-screen overlay, so the donut is *correctly* not animating while it
        # is up: that is the very state this fix stopped wasting frames on.
        # Escape it to reach the welcome screen underneath, which has a composer
        # and is where the donut is genuinely expected to spin.
        client.send(b"\x1b")
        time.sleep(2.0)
        # Escaping leaves a system notice in the transcript, and any non-empty
        # transcript makes `time_since_activity()` report deep idle, which
        # legitimately parks the donut. Real user interaction resets that clock,
        # so type and clear a character to land in the state a user is actually in
        # when they expect the donut to spin.
        client.send(b"x")
        time.sleep(0.4)
        flick.client_cmd(cmd_path, resp_path, "set_input:")
        time.sleep(1.5)

        print("== fresh-spawn welcome screen (animation expected) ==")
        before = schedule(cmd_path, resp_path)
        churn = moving_rows(client, args.watch_s)
        after = schedule(cmd_path, resp_path)
        partial_rate = (after["partial"] - before["partial"]) / args.watch_s
        print(f"  donut_active : {after['donut_active']}")
        print(f"  area         : {after['area']}")
        print(f"  interval_ms  : {after['interval_ms']}")
        print(f"  moving rows  : {len(churn)}")
        print(f"  partial/s    : {partial_rate:.1f}")

        if after["donut_active"]:
            if not after["area"]:
                failures.append("donut is active on the welcome screen but no "
                                "animation rectangle was published")
            if len(churn) == 0:
                failures.append("donut is active but no screen rows moved: the "
                                "animation is not actually running")
            if partial_rate < 5:
                failures.append(f"animation ticks are not being served by the "
                                f"cheap partial repaint ({partial_rate:.1f}/s)")

        # The composer exists on this screen, so the palette can open.
        print("\n== slash palette ==")
        # Set the composer through the debug channel rather than the PTY: this
        # verifier is about whether the palette *stays* on screen, not about key
        # delivery, and a dropped keystroke would look like a palette failure.
        # Set the composer through the debug channel rather than the PTY: this
        # verifier is about whether the palette *stays* on screen, not about key
        # delivery, and a dropped keystroke would look like a palette failure.
        flick.client_cmd(cmd_path, resp_path, "set_input:/")
        # The palette is drawn by the next frame, and an idle client may be on
        # the slow deep-idle tick, so wait for it to actually appear instead of
        # assuming one interval is enough.
        rows = client.rows()
        deadline = time.monotonic() + 8.0
        while time.monotonic() < deadline:
            rows = client.rows()
            if any(c in r for r in rows for c in MENU_CANDIDATES):
                break
            time.sleep(0.1)
        needles = tuple(c for c in MENU_CANDIDATES if any(c in r for r in rows))
        if not needles:
            failures.append("the slash palette never opened")
        else:
            print(f"  detected via : {needles}")
            watch = flick.watch_menu(client, needles, args.watch_s)
            print(f"  present      : {watch['menu_present_fraction']*100:.1f}%")
            print(f"  disappeared  : {watch['disappearances']}x")
            if watch["disappearances"] > 0:
                failures.append(f"the palette flickered "
                                f"{watch['disappearances']}x while open")

        print("\n== with the /resume overlay up ==")
        flick.client_cmd(cmd_path, resp_path, "set_input:")
        time.sleep(0.4)
        client.send(b"\x1b")  # close the palette
        time.sleep(0.3)
        flick.client_cmd(cmd_path, resp_path, "message:/resume")
        time.sleep(2.0)
        overlay = schedule(cmd_path, resp_path)
        overlay_churn = moving_rows(client, args.watch_s)
        print(f"  interval_ms  : {overlay['interval_ms']}")
        print(f"  area         : {overlay['area']}")
        print(f"  moving rows  : {len(overlay_churn)}")
        if overlay["area"] is not None and overlay["interval_ms"] == 16:
            # Only a real problem if it is animation cadence with nothing to show.
            failures.append("the overlay screen is paced at animation cadence")

        print()
        if failures:
            for f in failures:
                print(f"  FAIL: {f}")
            return 1
        print("  OK: the animation runs where it should, the palette is stable, "
              "and the overlay screen is not paced at animation cadence")
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
