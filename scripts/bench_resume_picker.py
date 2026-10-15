#!/usr/bin/env python3
"""Measure bare --resume first paint and populated-list latency using a real PTY.

Uses the current session store without selecting/resuming any session. Run against
both immutable baseline and candidate binaries with the same cwd and environment.
No third-party Python packages are required.
"""

import argparse
import fcntl
import json
import os
import pty
import re
import select
import signal
import statistics
import struct
import subprocess
import termios
import time

ANSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
POPULATED = re.compile(rb"\b[1-9][0-9]*\s+sessions\b")


def measure(binary, timeout, cancel_loading=False):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    env = dict(os.environ, TERM="xterm-256color", COLORTERM="truecolor")
    start = time.monotonic()
    proc = subprocess.Popen(
        [binary, "--resume", "--no-update"],
        stdin=slave, stdout=slave, stderr=slave, start_new_session=True, env=env,
    )
    os.close(slave)
    output = b""
    first_frame = populated = cancelled_at = None
    try:
        while time.monotonic() - start < timeout:
            ready, _, _ = select.select([master], [], [], 0.02)
            if ready:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                output += chunk
                # Reply to the optional terminal background query. Leaving it
                # unanswered adds a timeout unrelated to session scanning.
                for query, reply in (
                    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
                    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
                ):
                    if query in chunk:
                        os.write(master, reply)
                text = ANSI.sub(b"", output)
                elapsed = (time.monotonic() - start) * 1000
                if first_frame is None and b"(s/S filter)" in text:
                    first_frame = elapsed
                    if cancel_loading:
                        os.write(master, b"q")
                        cancelled_at = time.monotonic()
                if populated is None and POPULATED.search(text):
                    populated = elapsed
                    if not cancel_loading:
                        os.write(master, b"q")
                        cancelled_at = time.monotonic()
            if cancelled_at is not None and b"\x1b[?1049l" in output:
                break
            if proc.poll() is not None:
                break
        if proc.poll() is None:
            try:
                proc.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                # Older binaries fall through into agent startup after closing
                # the picker. Record that separately, then stop only our child.
                pass
        return {
            "first_frame_ms": first_frame,
            "populated_list_ms": populated,
            "cancel_exit_ms": None if cancelled_at is None else round(
                (time.monotonic() - cancelled_at) * 1000, 2
            ),
            "exit_code": proc.returncode,
            "terminal_restored": b"\x1b[?1049l" in output,
        }
    finally:
        if proc.poll() is None:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
        os.close(master)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--cancel-loading", action="store_true")
    args = parser.parse_args()
    samples = []
    for _ in range(args.runs):
        sample = measure(os.path.abspath(args.binary), args.timeout, args.cancel_loading)
        samples.append(sample)
        print(json.dumps(sample), flush=True)
    medians = {
        key: statistics.median(s[key] for s in samples if s[key] is not None)
        for key in ("first_frame_ms", "populated_list_ms", "cancel_exit_ms")
        if any(s[key] is not None for s in samples)
    }
    print(json.dumps({"median": medians, "runs": len(samples)}))
    if any(s["first_frame_ms"] is None or not s["terminal_restored"] for s in samples):
        raise SystemExit(1)
    if not args.cancel_loading and any(s["populated_list_ms"] is None for s in samples):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
