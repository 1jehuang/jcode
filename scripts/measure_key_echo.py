#!/usr/bin/env python3
"""Measure keystroke echo latency: first output byte vs the glyph reaching the screen.

A large gap between the two means the client responded promptly but the frame
carrying the character was not the one it drew, which is the animation-only
partial-repaint bug.
"""
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, "/home/jeremy/jcode/scripts")
import repro_input_flicker as R  # noqa: E402


def main() -> int:
    root = Path(tempfile.mkdtemp(prefix="jcode-echo-"))
    run = root / "run"
    run.mkdir(parents=True)
    env = os.environ.copy()
    env["JCODE_SOCKET"] = "/run/user/1000/jcode.sock"
    env["JCODE_DEBUG_CONTROL"] = "1"
    dbg = Path("/run/user/1000/jcode-debug.sock")
    cmd_p, resp_p = run / "cmd", run / "resp"

    binary = sys.argv[1] if len(sys.argv) > 1 else "./target/selfdev/jcode"
    sid = R.create_session(dbg, Path("/home/jeremy/jcode"))
    client = R.launch_client(binary, env, sid, cmd_p, resp_p)
    try:
        R.settle(cmd_p, resp_p)
        time.sleep(1.5)
        typed = ""
        for ch in "abcdefgh":
            typed += ch
            mark = client.history_mark()
            frames0 = client.frames
            t0 = time.monotonic()
            client.send_bytes(ch.encode())
            first_output = None
            shown = None
            while time.monotonic() - t0 < 1.5:
                time.sleep(0.002)
                if first_output is None and client.frames > frames0:
                    first_output = (time.monotonic() - t0) * 1000.0
                for at, val in client.history_since(mark):
                    if val is not None and typed in val:
                        shown = (at - t0) * 1000.0
                        break
                if shown is not None:
                    break
            fo = "?" if first_output is None else f"{first_output:.0f}ms"
            sh = "?" if shown is None else f"{shown:.0f}ms"
            print(f"  {typed!r}: first_output={fo}  composer_shows={sh}")
    finally:
        client.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
