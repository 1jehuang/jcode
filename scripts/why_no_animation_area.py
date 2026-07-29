#!/usr/bin/env python3
"""
Explain *why* the decorative idle animation has no recorded area on a freshly
spawned client, by asking the live client for its layout instead of inferring it
from source.

The animation-only fast path is disabled whenever `last_idle_animation_area()`
is `None`, and in that state the client still runs a full render on every 60fps
animation tick. This dumps the layout chunks and the animation area together so
the responsible term (donut rows squeezed to zero, or a size guard rejecting the
area) is identified from live state.
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


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--rows", type=int, default=48)
    ap.add_argument("--cols", type=int, default=160)
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-why-noarea-", dir=str(scratch)))
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
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-whynoarea")
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

        # Import here so the PTY size is configurable for this probe.
        import diagnose_idle_render_cost as diag
        diag.ROWS, diag.COLS = args.rows, args.cols
        client = launch(binary, env, sid, cmd_path, resp_path)
        if not settle(cmd_path, resp_path):
            print("client never came up")
            return 3
        time.sleep(3.0)

        # Visual-debug captures are produced by the *next* full frame, so
        # enabling and immediately asking for the layout reports "no frames
        # captured". Force a frame and give it time to land.
        # Captures are produced by the next *full* frame. The animation-only
        # partial repaint never calls `ui::draw`, so a client sitting on that
        # path produces no captures at all. A keystroke changes the composer,
        # which the fast path refuses to serve, forcing a real full frame.
        client_cmd(cmd_path, resp_path, "enable")
        client.send(b"x")
        time.sleep(1.0)
        client_cmd(cmd_path, resp_path, "set_input:")
        time.sleep(1.0)

        sched = (json.loads(client_cmd(cmd_path, resp_path, "draw-stats 1"))
                 .get("redraw_schedule") or {})
        print("== redraw schedule ==")
        print(json.dumps({k: sched.get(k) for k in (
            "idle_animation_active", "idle_animation_area", "interval_ms",
            "animation_fps", "tier", "periodic_redraw_required",
            "current_full_frame_reason")}, indent=2))

        print("\n== layout ==")
        layout = client_cmd(cmd_path, resp_path, "layout")
        print(layout[:3000])

        print("\n== screen (first 20 rows) ==")
        screen = client_cmd(cmd_path, resp_path, "screen-json")
        try:
            import json as _json
            rows = _json.loads(screen).get("rows") or []
            for i, row in enumerate(rows[:20]):
                text = row if isinstance(row, str) else row.get("text", "")
                print(f"{i:3} |{text[:120]}")
        except Exception:
            print(screen[:2000])
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
