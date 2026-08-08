#!/usr/bin/env python3
"""
Deterministic state prober for the fresh-spawn idle render loop.

Why this exists
---------------
Earlier measurement runs disagreed with each other: in one run the decorative
donut was active and in the next it was not, which makes every downstream number
untrustworthy. A measurement you cannot reproduce is not evidence. This prober
therefore *verifies* the preconditions (donut actually active, composer actually
holding the text we think it holds, suggestions actually non-empty) before it
reports any cost, and says so loudly when a precondition is unmet.

It also prints the exact fast-path decision inputs, so "the palette guard should
be firing" becomes a checkable claim rather than an inference from source.
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


def snapshot(cmd_path: Path, resp_path: Path) -> dict:
    ds = json.loads(client_cmd(cmd_path, resp_path, "draw-stats 1"))
    sched = ds.get("redraw_schedule") or {}
    anim = ds.get("idle_animation") or {}
    return {
        "donut_active": sched.get("idle_animation_active"),
        "interval_ms": sched.get("interval_ms"),
        "tier": sched.get("tier"),
        "animation_fps": sched.get("animation_fps"),
        "decorative": sched.get("decorative_animations"),
        "client_focused": sched.get("client_focused"),
        "periodic_required": sched.get("periodic_redraw_required"),
        "current_full_frame_reason": sched.get("current_full_frame_reason"),
        "status_notice": sched.get("status_notice"),
        "has_notification": sched.get("has_notification"),
        "animation_area": sched.get("idle_animation_area"),
        "partial": anim.get("partial_repaints") or 0,
        "full": anim.get("full_repaints") or 0,
        "blocked": dict(anim.get("fast_path_blocked") or {}),
        "key_to_paint": sched.get("key_to_paint"),
    }


def measure(client: Client, cmd_path: Path, resp_path: Path,
            label: str, window_s: float) -> dict:
    before = snapshot(cmd_path, resp_path)
    cpu0 = client.cpu_seconds()
    t0 = time.monotonic()
    time.sleep(window_s)
    elapsed = time.monotonic() - t0
    after = snapshot(cmd_path, resp_path)
    cpu1 = client.cpu_seconds()

    blocked_delta = {}
    for reason, count in after["blocked"].items():
        prev = before["blocked"].get(reason, 0)
        if count - prev > 0:
            blocked_delta[reason] = count - prev

    return {
        "state": label,
        "input": client_cmd(cmd_path, resp_path, "input").strip()[:60],
        "donut_active": after["donut_active"],
        "interval_ms": after["interval_ms"],
        "full_frames_per_s": round((after["full"] - before["full"]) / elapsed, 1),
        "partial_per_s": round((after["partial"] - before["partial"]) / elapsed, 1),
        "cpu_cores": (round((cpu1 - cpu0) / elapsed, 3)
                      if cpu0 is not None and cpu1 is not None else None),
        "blocked": blocked_delta,
        "full_frame_reason": after["current_full_frame_reason"],
        "status_notice": after["status_notice"],
        "has_notification": after["has_notification"],
        "client_focused": after["client_focused"],
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--window-s", type=float, default=3.0)
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-probe-", dir=str(scratch)))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    env.update({
        "JCODE_HOME": str(home), "JCODE_RUNTIME_DIR": str(run),
        "JCODE_SOCKET": str(run / "jcode.sock"), "JCODE_NO_TELEMETRY": "1",
        "JCODE_DEBUG_CONTROL": "1", "JCODE_TEMP_SERVER": "1",
        "JCODE_SERVER_OWNER_PID": str(os.getpid()), "JCODE_PERF_TIER": "full",
    })
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-probe")
    # Pin the theme so the client never issues an OSC 11 background query.
    # The client consumes that reply from stdin itself; a harness that also
    # answers it races the client and the leftover bytes get decoded as
    # composer keystrokes (observed as `]11;rgb:...` text in the input line),
    # which silently invalidates every measurement taken afterwards.
    env["JCODE_THEME"] = "dark"
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client: Client | None = None
    out: dict = {"binary": binary, "states": []}
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

        pre = snapshot(cmd_path, resp_path)
        out["preconditions"] = pre
        if not args.json:
            print("== preconditions ==")
            for key in ("donut_active", "tier", "animation_fps", "decorative",
                        "client_focused", "interval_ms", "animation_area",
                        "status_notice", "has_notification"):
                print(f"  {key:22}: {pre.get(key)}")
        if not pre["donut_active"]:
            out["error"] = ("donut inactive: this run cannot speak to the "
                            "reported fresh-spawn state")
            print(f"\n  ABORT: {out['error']}")
            return 3

        def run_state(label: str) -> None:
            state = measure(client, cmd_path, resp_path, label, args.window_s)
            out["states"].append(state)
            if not args.json:
                print(f"\n  [{label}] input={state['input']!r}")
                print(f"    full frames/s : {state['full_frames_per_s']}")
                print(f"    partial/s     : {state['partial_per_s']}")
                print(f"    cpu cores     : {state['cpu_cores']}")
                print(f"    donut active  : {state['donut_active']}")
                print(f"    interval ms   : {state['interval_ms']}")
                print(f"    blocked       : {json.dumps(state['blocked'])}")

        run_state("idle")

        # Type `/` through the real PTY so the composer state matches a user's.
        client.send(b"/")
        time.sleep(1.5)
        typed = client_cmd(cmd_path, resp_path, "input").strip()
        out["input_after_slash"] = typed
        if "/" not in typed:
            out["error"] = f"typing '/' did not reach the composer (input={typed!r})"
            print(f"\n  ABORT: {out['error']}")
            return 3
        run_state("palette-open")
        run_state("palette-held")

        client_cmd(cmd_path, resp_path, "set_input:hello")
        time.sleep(1.5)
        run_state("plain-draft")

        if args.json:
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
