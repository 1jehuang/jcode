#!/usr/bin/env python3
"""
Attribute the cost of a freshly spawned jcode's idle render loop.

Why this exists
---------------
The two other repro scripts prove *that* a fresh session is laggy and *that* the
slash menu blinks. Neither says *which* repaint is expensive, and the aggregate
`draw-stats` summary hides it: a p50 of 3ms with a 695ms max averages out to
"41ms", which is not attributable to anything.

This script watches the live client while it walks through the states a user
actually hits on a fresh spawn, and for each state reports:

  * full draws per second (`terminal.draw` calls, the expensive path)
  * partial animation repaints per second (the cheap path)
  * the client process's CPU time consumed in that state

CPU time is the honest measure of "laggy": a client burning a whole core on
decorative repaints has no headroom left for a keystroke, regardless of how good
the median frame looks.

States exercised
----------------
  idle            : fresh session, nothing typed (donut spinning)
  palette-open    : `/` typed and left open past the typing backoff window
  palette-settled : same, sampled later, to catch a steady state
  plain-draft     : ordinary text left in the composer

Usage
-----
  python3 scripts/diagnose_idle_render_cost.py [--binary PATH] [--json]
"""
from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import fcntl
import struct
import termios
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
ROWS, COLS = 48, 160

_TERM_REPLIES = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[c", b"\x1b[?62;c"),
    (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
    (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
    (b"\x1b[14t", b"\x1b[4;600;800t"),
    (b"\x1b[16t", b"\x1b[6;16;8t"),
    (b"\x1b[18t", f"\x1b[8;{ROWS};{COLS}t".encode()),
    (b"\x1b[?1016$p", b"\x1b[?1016;1$y"),
    (b"\x1b[?2027$p", b"\x1b[?2027;1$y"),
    (b"\x1b[?2031$p", b"\x1b[?2031;1$y"),
    (b"\x1b[?1004$p", b"\x1b[?1004;1$y"),
    (b"\x1b[?2004$p", b"\x1b[?2004;1$y"),
    (b"\x1b[?2026$p", b"\x1b[?2026;1$y"),
    (b"\x1b[?u", b"\x1b[?3u"),
]


def wait_for_socket(path: Path, timeout_s: float = 30.0) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if path.exists():
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.settimeout(0.2)
                s.connect(str(path))
                s.close()
                return
            except OSError:
                pass
        time.sleep(0.02)
    raise RuntimeError(f"socket not ready: {path}")


def _recv(sock: socket.socket, timeout: float) -> dict:
    sock.settimeout(timeout)
    buf = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            line = line.strip()
            if not line:
                continue
            resp = json.loads(line.decode())
            if resp.get("type") in ("ack", "pong"):
                continue
            return resp
    raise RuntimeError("debug socket closed without a response")


def dbg(debug_sock: Path, command: str, timeout: float = 30.0) -> str:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(str(debug_sock))
    try:
        s.sendall((json.dumps({"type": "debug_command", "id": 1,
                               "command": command}) + "\n").encode())
        resp = _recv(s, timeout)
    finally:
        s.close()
    if resp.get("type") == "error":
        raise RuntimeError(f"debug error for {command!r}: {resp.get('message')}")
    return resp.get("output", "")


def client_cmd(cmd_path: Path, resp_path: Path, command: str,
               timeout_s: float = 10.0) -> str:
    try:
        resp_path.unlink()
    except FileNotFoundError:
        pass
    cmd_path.write_text(command)
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if resp_path.exists():
            time.sleep(0.02)
            return resp_path.read_text()
        time.sleep(0.01)
    raise RuntimeError(f"client did not answer {command!r} in {timeout_s}s")


@dataclass
class Client:
    proc: subprocess.Popen
    master_fd: int
    stop: threading.Event = field(default_factory=threading.Event)
    thread: threading.Thread | None = None

    def _pump(self) -> None:
        probe = b""
        while not self.stop.is_set():
            try:
                rlist, _, _ = select.select([self.master_fd], [], [], 0.05)
            except (OSError, ValueError):
                break
            if not rlist:
                if self.proc.poll() is not None:
                    break
                continue
            try:
                chunk = os.read(self.master_fd, 65536)
            except (BlockingIOError, OSError):
                continue
            if not chunk:
                break
            probe = (probe + chunk)[-8192:]
            changed = True
            while changed:
                changed = False
                for query, response in _TERM_REPLIES:
                    if query in probe:
                        try:
                            os.write(self.master_fd, response)
                        except OSError:
                            pass
                        probe = probe.replace(query, b"")
                        changed = True

    def start_pump(self) -> None:
        self.thread = threading.Thread(target=self._pump, daemon=True)
        self.thread.start()

    def send(self, data: bytes) -> None:
        os.write(self.master_fd, data)

    def cpu_seconds(self) -> float | None:
        """utime+stime of the client process, in seconds."""
        try:
            fields = Path(f"/proc/{self.proc.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        except (OSError, IndexError):
            return None
        try:
            utime, stime = int(fields[11]), int(fields[12])
        except (IndexError, ValueError):
            return None
        return (utime + stime) / os.sysconf("SC_CLK_TCK")

    def shutdown(self) -> None:
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=1.0)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(self.proc.pid, sig)
                self.proc.wait(timeout=2.0)
                break
            except (ProcessLookupError, PermissionError):
                break
            except Exception:
                continue
        try:
            os.close(self.master_fd)
        except OSError:
            pass


def launch(binary: str, env: dict, session_id: str,
           cmd_path: Path, resp_path: Path) -> Client:
    master_fd, slave_fd = pty.openpty()
    fcntl.ioctl(slave_fd, termios.TIOCSWINSZ,
                struct.pack("HHHH", ROWS, COLS, 0, 0))
    cenv = dict(env)
    cenv["JCODE_DEBUG_CMD_PATH"] = str(cmd_path)
    cenv["JCODE_DEBUG_RESPONSE_PATH"] = str(resp_path)
    cenv["TERM"] = "xterm-256color"
    proc = subprocess.Popen(
        [binary, "--no-update", "--no-selfdev",
         "--socket", env["JCODE_SOCKET"], "--resume", session_id],
        stdin=slave_fd, stdout=slave_fd, stderr=slave_fd,
        env=cenv, preexec_fn=os.setsid,
    )
    os.close(slave_fd)
    os.set_blocking(master_fd, False)
    client = Client(proc=proc, master_fd=master_fd)
    client.start_pump()
    return client


def settle(cmd_path: Path, resp_path: Path, timeout_s: float = 60.0) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            client_cmd(cmd_path, resp_path, "input", timeout_s=2.0)
            return True
        except Exception:
            time.sleep(0.2)
    return False


def counters(cmd_path: Path, resp_path: Path) -> dict:
    payload = json.loads(client_cmd(cmd_path, resp_path, "draw-stats 1"))
    anim = payload.get("idle_animation") or {}
    sched = payload.get("redraw_schedule") or {}
    return {
        "partial": anim.get("partial_repaints") or 0,
        "full": anim.get("full_repaints") or 0,
        "buffered": payload.get("buffered_samples") or 0,
        "blocked": anim.get("fast_path_blocked") or {},
        "interval_ms": sched.get("interval_ms"),
        "donut_active": sched.get("idle_animation_active"),
        "periodic_required": sched.get("periodic_redraw_required"),
        "full_frame_reason": sched.get("current_full_frame_reason"),
        "key_to_paint": sched.get("key_to_paint"),
    }


def probe_state(client: Client, cmd_path: Path, resp_path: Path,
                label: str, window_s: float) -> dict:
    """Measure draw rates and CPU burn over a window in the current UI state."""
    before = counters(cmd_path, resp_path)
    cpu0 = client.cpu_seconds()
    t0 = time.monotonic()
    time.sleep(window_s)
    elapsed = time.monotonic() - t0
    after = counters(cmd_path, resp_path)
    cpu1 = client.cpu_seconds()

    def delta(key: str) -> int:
        return max(0, (after.get(key) or 0) - (before.get(key) or 0))

    # `full` is a monotonic counter of full frames, unlike `buffered` which is a
    # capped ring buffer whose delta saturates at 240 and silently understates
    # a busy loop.
    full_draws = delta("full")

    blocked_delta = {}
    for reason, count in (after.get("blocked") or {}).items():
        prev = (before.get("blocked") or {}).get(reason, 0)
        if count - prev > 0:
            blocked_delta[reason] = count - prev

    cpu_ratio = None
    if cpu0 is not None and cpu1 is not None:
        cpu_ratio = round((cpu1 - cpu0) / elapsed, 3)

    return {
        "state": label,
        "window_s": round(elapsed, 2),
        # Full frames are the expensive path (~50x a partial repaint).
        "full_draws_per_s": round(full_draws / elapsed, 1),
        "sampled_draws_per_s": round(delta("buffered") / elapsed, 1),
        "partial_repaints_per_s": round(delta("partial") / elapsed, 1),
        "cpu_cores": cpu_ratio,
        "redraw_interval_ms": after.get("interval_ms"),
        "donut_active": after.get("donut_active"),
        "periodic_redraw_required": after.get("periodic_required"),
        "full_frame_reason": after.get("full_frame_reason"),
        "fast_path_blocked": blocked_delta,
    }


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    default_bin = REPO_ROOT / "target" / "selfdev" / "jcode"
    if not default_bin.exists():
        default_bin = Path.home() / ".jcode" / "builds" / "current" / "jcode"
    ap.add_argument("--binary", default=str(default_bin))
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--window-s", type=float, default=2.0)
    ap.add_argument("--no-idle-animation", action="store_true")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"binary not found: {binary}")
        return 3

    root = Path(tempfile.mkdtemp(prefix="jcode-render-cost-"))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    env["JCODE_HOME"] = str(home)
    env["JCODE_RUNTIME_DIR"] = str(run)
    env["JCODE_SOCKET"] = str(run / "jcode.sock")
    env["JCODE_NO_TELEMETRY"] = "1"
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_TEMP_SERVER"] = "1"
    env["JCODE_SERVER_OWNER_PID"] = str(os.getpid())
    env.setdefault("JCODE_PERF_TIER", "full")
    # Pin the theme so the client never issues an OSC 11 background query.
    # The client consumes that reply from stdin itself; a harness that also
    # answers it races the client and the leftover bytes get decoded as
    # composer keystrokes (observed as `]11;rgb:...` text in the input line),
    # which silently invalidates every measurement taken afterwards.
    env["JCODE_THEME"] = "dark"
    if args.no_idle_animation:
        env["JCODE_IDLE_ANIMATION"] = "false"
    if not env.get("ANTHROPIC_API_KEY"):
        env["ANTHROPIC_API_KEY"] = "sk-ant-diagnose-render-cost"
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    if not args.json:
        print("== jcode idle render cost ==")
        print(f"  binary: {binary}")
        print(f"  donut : {'off' if args.no_idle_animation else 'on'}")

    log_fh = (root / "server.log").open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client: Client | None = None
    result: dict = {"binary": binary,
                    "idle_animation": not args.no_idle_animation,
                    "states": []}
    try:
        wait_for_socket(Path(env["JCODE_SOCKET"]))
        wait_for_socket(debug_sock)
        session_id = dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        if session_id.startswith("{"):
            session_id = json.loads(session_id).get("session_id", "")
        session_id = session_id.split()[-1] if session_id else ""
        if not session_id:
            print("could not create a session")
            return 3

        client = launch(binary, env, session_id, cmd_path, resp_path)
        if not settle(cmd_path, resp_path):
            print("client never came up")
            return 3
        # Let startup churn (catalog, auth probes) drain so steady state is clean.
        time.sleep(3.0)

        def record(label: str) -> dict:
            state = probe_state(client, cmd_path, resp_path, label, args.window_s)
            result["states"].append(state)
            if not args.json:
                print(f"\n  [{label}]")
                print(f"    full draws/s  : {state['full_draws_per_s']}")
                print(f"    partial/s     : {state['partial_repaints_per_s']}")
                print(f"    cpu cores     : {state['cpu_cores']}")
                print(f"    redraw ms     : {state['redraw_interval_ms']}")
                print(f"    donut active  : {state['donut_active']}")
                print(f"    blocked       : {json.dumps(state['fast_path_blocked'])}")
            return state

        record("idle")

        # Open the slash palette and let the typing backoff window lapse, which
        # is the state a user is in while reading the command list.
        client.send(b"/")
        time.sleep(1.2)
        record("palette-open")
        record("palette-settled")

        # An ordinary draft: same composer-non-empty condition, no overlay.
        client_cmd(cmd_path, resp_path, "set_input:hello")
        time.sleep(1.2)
        record("plain-draft")

        client_cmd(cmd_path, resp_path, "set_input:")
        time.sleep(1.2)
        record("idle-again")

        if args.json:
            print(json.dumps(result, indent=2))
        else:
            worst = max(result["states"], key=lambda s: s["full_draws_per_s"])
            print(f"\n  worst state: {worst['state']} at "
                  f"{worst['full_draws_per_s']} full draws/s, "
                  f"{worst['cpu_cores']} cores")
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
