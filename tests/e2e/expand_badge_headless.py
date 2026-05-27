#!/usr/bin/env python3
"""Headless E2E regression for the edit expand badge shortcut.

This starts a real jcode TUI client inside a pseudo-terminal, prepares a real
rendered edit-diff expand-badge fixture through the client debug command, sends
terminal key bytes to the PTY, and asserts the live TUI state changed.

Run from repo root after building jcode:
  python3 tests/e2e/expand_badge_headless.py
"""

import json
import os
import pty
import select
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

RUNTIME_DIR = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
SOCKET_PATH = os.path.join(RUNTIME_DIR, "jcode-debug.sock")
REPO = Path(__file__).resolve().parents[2]


def send_cmd(sock, cmd, session_id=None, timeout=10):
    req = {"type": "debug_command", "id": int(time.time() * 1000) % 1000000, "command": cmd}
    if session_id:
        req["session_id"] = session_id
    sock.sendall((json.dumps(req) + "\n").encode())
    sock.settimeout(timeout)
    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            raise RuntimeError("debug socket closed")
        data += chunk
        try:
            return json.loads(data.decode())
        except json.JSONDecodeError:
            continue


def wait_for(predicate, timeout=10, interval=0.1, desc="condition"):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(interval)
    raise AssertionError(f"timed out waiting for {desc}; last={last!r}")


def drain_pty(master_fd):
    chunks = []
    while True:
        r, _, _ = select.select([master_fd], [], [], 0)
        if not r:
            break
        try:
            chunks.append(os.read(master_fd, 65536))
        except OSError:
            break
    return b"".join(chunks)


def main():
    binary = os.environ.get("JCODE_E2E_BIN")
    if not binary:
        candidates = [
            REPO / "target" / "selfdev" / "jcode",
            Path.home() / ".jcode" / "builds" / "current" / "jcode",
        ]
        binary = next((str(p) for p in candidates if p.exists()), None)
    if not binary:
        raise SystemExit("No jcode binary found. Set JCODE_E2E_BIN or build first.")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(SOCKET_PATH)

    session_id = None
    proc = None
    master_fd = None
    try:
        res = send_cmd(sock, f"create_session:selfdev:{REPO}")
        if not res.get("ok"):
            raise AssertionError(f"create_session failed: {res}")
        session_id = json.loads(res["output"])["session_id"]
        print(f"session={session_id}")

        master_fd, slave_fd = pty.openpty()
        env = os.environ.copy()
        env.setdefault("TERM", "xterm-kitty")
        env.setdefault("JCODE_CLIENT_SELFDEV_MODE", "1")
        proc = subprocess.Popen(
            [binary, "self-dev", "--resume", session_id],
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            cwd=str(REPO),
            env=env,
            start_new_session=True,
        )
        os.close(slave_fd)

        def client_attached():
            out = send_cmd(sock, "clients:map", timeout=5)
            clients = json.loads(out["output"])["clients"] if isinstance(out.get("output"), str) else out.get("clients", [])
            return any(c.get("session_id") == session_id for c in clients)

        wait_for(client_attached, timeout=15, desc="TUI client attach")
        drain_pty(master_fd)

        res = send_cmd(sock, "client:expand-badge-fixture", session_id=session_id)
        if not res.get("ok"):
            raise AssertionError(f"fixture failed: {res}")
        print(f"fixture={res['output']}")
        time.sleep(0.5)
        screen = send_cmd(sock, "client:history", session_id=session_id).get("output", "")
        if "Edited demo.txt" not in screen:
            raise AssertionError("fixture did not install edit message")

        state = json.loads(send_cmd(sock, "client:expand-badge-state", session_id=session_id)["output"])
        print(f"before={state}")
        assert state["diff_mode"] == "Inline", state
        assert state["input"] == "", state

        # CSI-u for Alt+Shift+e: codepoint 101 ('e'), modifier value 4
        # (1 + shift bit 1 + alt bit 2). This is the enhanced keyboard
        # protocol path used by modern terminals and crossterm.
        os.write(master_fd, b"\x1b[101;4u")
        time.sleep(0.5)

        after = json.loads(send_cmd(sock, "client:expand-badge-state", session_id=session_id)["output"])
        print(f"after_csi_u={after}")
        if after["diff_mode"] != "FullInline" or after["input"]:
            # Also try the legacy Alt+e encoding so failures print both states.
            os.write(master_fd, b"\x1be")
            time.sleep(0.5)
            after_legacy = json.loads(send_cmd(sock, "client:expand-badge-state", session_id=session_id)["output"])
            print(f"after_legacy={after_legacy}")
            raise AssertionError(
                "Alt+Shift+E did not expand in headless PTY: "
                f"after_csi_u={after}, after_legacy={after_legacy}"
            )

        print("PASS: headless PTY Alt+Shift+E expands edit badge without inserting text")
        return 0
    finally:
        if proc and proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except Exception:
                proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
        if master_fd is not None:
            try:
                os.close(master_fd)
            except OSError:
                pass
        if session_id:
            try:
                send_cmd(sock, f"destroy_session:{session_id}", timeout=5)
            except Exception:
                pass
        sock.close()


if __name__ == "__main__":
    raise SystemExit(main())
