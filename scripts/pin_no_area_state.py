#!/usr/bin/env python3
"""
Pin down the live state in which the redraw loop ticks at animation cadence
while the renderer publishes no animation rectangle.

Unit tests with a synthetic `TuiState` do *not* reproduce it: there the
onboarding path publishes an 18-row rectangle as expected. So the live client is
in a state the synthetic state does not model. This samples the live client
repeatedly and correlates `idle_animation_area` with everything else the client
will tell us, so the distinguishing field is found by observation.
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

WATCH_KEYS = (
    "idle_animation_active", "idle_animation_area", "interval_ms",
    "periodic_redraw_required", "current_full_frame_reason",
    "client_focused", "has_notification", "status_notice",
)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary",
                    default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--samples", type=int, default=14)
    ap.add_argument("--interval-s", type=float, default=1.0)
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-pinstate-", dir=str(scratch)))
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
    env.setdefault("ANTHROPIC_API_KEY", "sk-ant-pinstate")
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

        print(f"{'t':>6} {'donut':>6} {'area':>22} {'ms':>4} {'msgs':>5} "
              f"{'proc':>5} {'notif':>5} draws/s")
        prev_count = None
        prev_ts = None
        t0 = time.monotonic()
        for _ in range(args.samples):
            try:
                ds = json.loads(client_cmd(cmd_path, resp_path, "draw-stats 240"))
                state = json.loads(client_cmd(cmd_path, resp_path, "state"))
            except Exception as e:  # noqa: BLE001
                print(f"  sample failed: {e}")
                time.sleep(args.interval_s)
                continue
            sched = ds.get("redraw_schedule") or {}
            samples = ds.get("samples") or []
            now = time.monotonic()
            rate = ""
            if samples and prev_ts is not None:
                fresh = [s for s in samples if s["timestamp_ms"] > prev_ts]
                rate = f"{len(fresh) / (now - prev_count):.1f}" if prev_count else ""
            if samples:
                prev_ts = samples[-1]["timestamp_ms"]
            prev_count = now
            area = sched.get("idle_animation_area")
            print(f"{now - t0:6.1f} {str(sched.get('idle_animation_active')):>6} "
                  f"{str(area):>22} {str(sched.get('interval_ms')):>4} "
                  f"{str(state.get('display_messages')):>5} "
                  f"{str(state.get('processing')):>5} "
                  f"{str(sched.get('has_notification')):>5} {rate}")
            time.sleep(args.interval_s)
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
