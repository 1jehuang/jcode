#!/usr/bin/env python3
"""
Profile a client spawned into the user's *real* environment.

`repro_real_spawn_lag.py` showed the shape of the problem: 0.3 CPU cores burned
while `draws_per_s` reads 0.0. Those are not full frames, so `draw-stats` cannot
see them: the animation-only partial repaint path deliberately skips
`record_draw_call_attribution`. `perf` sees the work regardless of which path
does it, so this attributes the burn on a real session.
"""
from __future__ import annotations

import argparse, json, os, signal, subprocess, sys, tempfile, time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))
import repro_slash_flicker as flick  # noqa: E402
from repro_real_spawn_lag import recent_session  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--binary", default=str(REPO_ROOT / "target" / "selfdev" / "jcode"))
    ap.add_argument("--session", default=None)
    ap.add_argument("--seconds", type=float, default=8.0)
    ap.add_argument("--freq", type=int, default=997)
    args = ap.parse_args()

    flick.ROWS, flick.COLS = 48, 160
    binary = str(Path(args.binary).resolve())
    runtime = Path(os.environ.get("JCODE_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
    env = os.environ.copy()
    env["JCODE_SOCKET"] = env.get("JCODE_SOCKET") or str(runtime / "jcode.sock")
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_THEME"] = "dark"
    debug_sock = runtime / "jcode-debug.sock"

    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR") or tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="jcode-profile-real-", dir=str(scratch)))
    cmd_path, resp_path = root / "client_cmd", root / "client_resp"

    session = args.session or recent_session(debug_sock, str(REPO_ROOT))
    if not session:
        session = flick.dbg(debug_sock, f"create_session:{REPO_ROOT}").strip().split()[-1]
    print(f"== profiling real spawn ==\n  binary : {binary}\n  session: {session}")

    client = None
    try:
        client = flick.launch(binary, env, session, cmd_path, resp_path)
        if not flick.settle(cmd_path, resp_path, timeout_s=90.0):
            print("client never came up")
            return 3
        time.sleep(3.0)
        sched = (json.loads(flick.client_cmd(cmd_path, resp_path, "draw-stats 1"))
                 .get("redraw_schedule") or {})
        print(f"  donut={sched.get('idle_animation_active')} "
              f"area={sched.get('idle_animation_area')} "
              f"interval={sched.get('interval_ms')}ms")

        data = root / "perf.data"
        print(f"  sampling {args.seconds}s at {args.freq}Hz ...")
        rec = subprocess.run(["perf", "record", "-F", str(args.freq), "-g",
                              "--pid", str(client.proc.pid), "-o", str(data),
                              "--", "sleep", str(args.seconds)],
                             capture_output=True, text=True)
        if rec.returncode != 0 or not data.exists():
            print("  perf record failed:"); print((rec.stderr or rec.stdout)[-1500:]); return 3
        rep = subprocess.run(["perf", "report", "-i", str(data), "--stdio",
                              "--no-children", "--percent-limit", "1.0", "-g", "none"],
                             capture_output=True, text=True)
        print("\n=== self time ===")
        for line in rep.stdout.splitlines():
            if line.strip() and not line.startswith("#"):
                print(line[:150])
        return 0
    finally:
        if client:
            client.shutdown()
        import shutil
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
