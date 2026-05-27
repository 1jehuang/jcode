#!/usr/bin/env python3
"""Headed compositor smoke for Alt+Shift+E expand badge.

Uses kitty remote control to launch a real jcode TUI window, prepares the same
fixture, focuses the window, presses Alt+Shift+E with wtype, and checks state.
This complements the headless PTY E2E by covering Wayland/compositor mapping.
"""

import json
import os
import socket
import subprocess
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
            pass


def sh(cmd, **kwargs):
    return subprocess.check_output(cmd, text=True, **kwargs).strip()


def wait_for(fn, timeout=10, desc="condition"):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = fn()
        if last:
            return last
        time.sleep(0.15)
    raise AssertionError(f"timed out waiting for {desc}; last={last!r}")


def find_kitty_socket():
    for path in sorted(Path("/tmp").glob("kitty.sock-*")):
        try:
            out = sh(["kitty", "@", "--to", f"unix:{path}", "ls"], stderr=subprocess.DEVNULL)
            data = json.loads(out)
            if data:
                return str(path)
        except Exception:
            continue
    raise SystemExit("No kitty remote socket found")


def find_niri_window_id(title):
    try:
        out = sh(["niri", "msg", "windows"])
    except Exception:
        return None
    current_id = None
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("Window ID "):
            current_id = line.split()[2].rstrip(":")
        elif line.startswith('Title: ') and title in line and current_id:
            return current_id
    return None


def focused_niri_window_id():
    try:
        out = sh(["niri", "msg", "windows"])
    except Exception:
        return None
    current_id = None
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("Window ID "):
            current_id = line.split()[2].rstrip(":")
            if "(focused)" in line:
                return current_id
    return None


def main():
    if not shutil_which("kitty") or not shutil_which("wtype"):
        raise SystemExit("SKIP: kitty and wtype are required")
    binary = os.environ.get("JCODE_E2E_BIN", str(REPO / "target" / "selfdev" / "jcode"))
    kitty_sock = os.environ.get("KITTY_E2E_SOCKET", find_kitty_socket())
    title = f"JCODE_EXPAND_E2E_{int(time.time())}"

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(SOCKET_PATH)
    session_id = None
    window_id = None
    previous_focused_niri = focused_niri_window_id()
    try:
        res = send_cmd(sock, f"create_session:selfdev:{REPO}")
        if not res.get("ok"):
            raise AssertionError(res)
        session_id = json.loads(res["output"])["session_id"]
        print(f"session={session_id}")

        window_id = sh([
            "kitty", "@", "--to", f"unix:{kitty_sock}", "launch",
            "--type", "os-window", "--title", title,
            binary, "self-dev", "--resume", session_id,
        ])
        print(f"window={window_id}")

        def attached():
            out = send_cmd(sock, "clients:map", timeout=5)
            clients = json.loads(out["output"])["clients"] if isinstance(out.get("output"), str) else out.get("clients", [])
            return any(c.get("session_id") == session_id for c in clients)
        wait_for(attached, timeout=15, desc="client attach")

        res = send_cmd(sock, "client:expand-badge-fixture", session_id=session_id)
        print(f"fixture={res.get('output')}")
        before = json.loads(send_cmd(sock, "client:expand-badge-state", session_id=session_id)["output"])
        print(f"before={before}")

        niri_id = find_niri_window_id(title)
        focused_after_launch = focused_niri_window_id()
        if niri_id is None and focused_after_launch != previous_focused_niri:
            niri_id = focused_after_launch
        if niri_id is None:
            niri_id = wait_for(lambda: find_niri_window_id(title), timeout=3, desc="niri os-window")
        print(f"niri_window={niri_id}")
        subprocess.run(["niri", "msg", "action", "focus-window", "--id", str(niri_id)], check=False)
        sh(["kitty", "@", "--to", f"unix:{kitty_sock}", "focus-window", "--match", f"id:{window_id}"])
        time.sleep(0.5)
        subprocess.check_call(["wtype", "-M", "alt", "-M", "shift", "e", "-m", "shift", "-m", "alt"])
        time.sleep(0.5)

        after = json.loads(send_cmd(sock, "client:expand-badge-state", session_id=session_id)["output"])
        print(f"after={after}")
        if after["diff_mode"] != "FullInline" or after["input"]:
            raise AssertionError(f"headed wtype Alt+Shift+E failed: {after}")
        print("PASS: headed wtype Alt+Shift+E expands edit badge")
    finally:
        if window_id:
            subprocess.run(["kitty", "@", "--to", f"unix:{kitty_sock}", "close-window", "--match", f"id:{window_id}"], stderr=subprocess.DEVNULL)
        if session_id:
            try:
                send_cmd(sock, f"destroy_session:{session_id}", timeout=5)
            except Exception:
                pass
        sock.close()


def shutil_which(cmd):
    from shutil import which
    return which(cmd)


if __name__ == "__main__":
    main()
