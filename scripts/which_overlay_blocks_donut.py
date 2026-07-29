#!/usr/bin/env python3
"""
Identify the exact UI state a freshly spawned jcode client sits in while its
render loop runs at animation cadence with no animation area recorded.

`ui::draw_inner` returns early for a set of full-screen overlays (changelog,
help, model status, session picker, login picker, account picker). Every one of
those returns happens *after* `record_idle_animation_area(None)` and *before* the
donut is laid out, so in that state:

  * `idle_donut_active()` is true, so the loop ticks at animation FPS, and
  * `last_idle_animation_area()` is None, so the cheap animation-only repaint is
    refused ("no_animation_area"),

which means every one of those ~60 ticks per second becomes a full render. This
script asks the live client which overlay is up, so the responsible state is
named rather than guessed.
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

# Debug commands that reveal overlay state, tried in order.
PROBES = ("picker", "state", "overlays", "screen-text", "screen", "help")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--fresh-session", action="store_true",
                    help="start a brand new session instead of resuming one")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-whichoverlay-", dir=str(scratch)))
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
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-whichoverlay")
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client: Client | None = None
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

        sched = (json.loads(client_cmd(cmd_path, resp_path, "draw-stats 1"))
                 .get("redraw_schedule") or {})
        print("== redraw schedule ==")
        print(json.dumps({k: sched.get(k) for k in (
            "idle_animation_active", "idle_animation_area", "interval_ms")},
            indent=2))

        for probe in PROBES:
            try:
                answer = client_cmd(cmd_path, resp_path, probe, timeout_s=6.0)
            except Exception as e:  # noqa: BLE001
                print(f"\n== {probe} == (failed: {e})")
                continue
            print(f"\n== {probe} ==")
            print(answer[:1500])
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
