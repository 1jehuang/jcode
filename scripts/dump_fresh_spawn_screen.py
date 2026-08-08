#!/usr/bin/env python3
"""
Dump the rendered screen of a freshly spawned jcode client through a real
terminal emulator, and report which rows change over time.

This answers a question the counters cannot: on the fresh-spawn screen, is the
decorative animation actually visible? `count_idle_draws.py` showed 62.7 full
renders per second with `changed_cells` pinned at 0, which implies nothing at all
was animating. Seeing the actual screen confirms whether the donut is absent
(so the 60fps cadence is pure waste) or present (so the cost is real work).
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


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--rows", type=int, default=48)
    ap.add_argument("--cols", type=int, default=160)
    ap.add_argument("--watch-s", type=float, default=2.0)
    ap.add_argument("--type-slash", action="store_true")
    args = ap.parse_args()

    flick.ROWS, flick.COLS = args.rows, args.cols
    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-screendump-", dir=str(scratch)))
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
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-screendump")
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client = None
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

        if args.type_slash:
            client.send(b"/")
            time.sleep(1.0)

        sched = (json.loads(flick.client_cmd(cmd_path, resp_path, "draw-stats 1"))
                 .get("redraw_schedule") or {})
        print("== schedule ==")
        print(json.dumps({k: sched.get(k) for k in (
            "idle_animation_active", "idle_animation_area", "interval_ms")},
            indent=2))

        print(f"\n== screen {args.cols}x{args.rows} ==")
        rows = client.rows()
        for i, row in enumerate(rows):
            print(f"{i:3} |{row.rstrip()[:150]}")

        # Which rows move? A visible animation churns its own rows every frame.
        churn: dict[int, int] = {}
        prev = rows
        t0 = time.monotonic()
        while time.monotonic() - t0 < args.watch_s:
            cur = client.rows()
            for idx, (a, b) in enumerate(zip(prev, cur)):
                if a != b:
                    churn[idx] = churn.get(idx, 0) + 1
            prev = cur
            time.sleep(0.005)
        print(f"\n== rows that changed over {args.watch_s}s ==")
        if churn:
            for idx, count in sorted(churn.items()):
                print(f"  row {idx:3}: {count} changes")
        else:
            print("  NONE: the screen is completely static")
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
