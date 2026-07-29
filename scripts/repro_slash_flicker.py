#!/usr/bin/env python3
"""
Reproduce the "slash command menu flickers in a freshly spawned jcode" bug.

Why this exists
---------------
`repro_input_lag.py` measures keystroke latency. It cannot see *flicker*,
because flicker is not a latency: the byte arrives fast, but the menu it drew
is erased by a later repaint and then drawn again. Measuring bytes cannot tell
those apart, so this script feeds the client's real output into a terminal
emulator (`pyte`) and watches the *rendered screen* over time.

What it measures
----------------
After typing `/` (which opens the slash-command suggestion popup) the popup
must stay on screen continuously. We snapshot the emulated screen every few
milliseconds and count transitions:

    present -> absent   = the menu was erased (a flicker)
    absent  -> present  = it came back

A stable menu yields 1 transition (absent -> present, when it first opens).
Anything more means the user sees blinking, which is exactly the reported bug.

The same emulator also detects *transcript* flicker: on a fresh session the
header/tips block is static, so any row above the composer that vanishes and
returns is a visible artifact.

Usage
-----
  python3 scripts/repro_slash_flicker.py [--binary PATH] [--json] [-v]

Exit codes:
  0 = no flicker observed
  1 = flicker reproduced
  3 = setup failure
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

import pyte

REPO_ROOT = Path(__file__).resolve().parent.parent
ROWS, COLS = 48, 160

# Terminal capability replies, so the client's startup probes do not stall.
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
               timeout_s: float = 8.0) -> str:
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
class Screen:
    """A live pyte emulation of the client's terminal.

    Feeding the client's real output through an emulator is what makes flicker
    observable: the bug is a *rendered* state that disappears and returns, which
    no byte-level measurement can distinguish from a normal repaint.
    """
    proc: subprocess.Popen
    master_fd: int
    stop: threading.Event = field(default_factory=threading.Event)
    thread: threading.Thread | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)
    screen: pyte.Screen = field(default_factory=lambda: pyte.Screen(COLS, ROWS))
    stream: pyte.Stream | None = None
    output_events: list[float] = field(default_factory=list)
    total_bytes: int = 0

    def __post_init__(self) -> None:
        self.stream = pyte.Stream(self.screen)

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
            now = time.monotonic()
            with self.lock:
                self.output_events.append(now)
                self.total_bytes += len(chunk)
                self.stream.feed(chunk.decode("utf-8", "replace"))
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

    def rows(self) -> list[str]:
        with self.lock:
            return list(self.screen.display)

    def send(self, data: bytes) -> None:
        os.write(self.master_fd, data)

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
           cmd_path: Path, resp_path: Path) -> Screen:
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
    client = Screen(proc=proc, master_fd=master_fd)
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


# ── flicker detection ────────────────────────────────────────────────────────
def menu_rows(rows: list[str], needles: tuple[str, ...]) -> int:
    """How many rows currently show slash-command menu entries."""
    return sum(1 for r in rows if any(n in r for n in needles))


def watch_menu(client: Screen, needles: tuple[str, ...], seconds: float,
               verbose: bool = False) -> dict:
    """Sample the rendered screen and count menu present/absent transitions."""
    samples: list[tuple[float, int]] = []
    t0 = time.monotonic()
    while time.monotonic() - t0 < seconds:
        rows = client.rows()
        samples.append((time.monotonic() - t0, menu_rows(rows, needles)))
        time.sleep(0.004)

    transitions = []
    prev = None
    for ts, count in samples:
        present = count > 0
        if prev is None:
            prev = present
            if present:
                transitions.append((ts, "appear"))
            continue
        if present != prev:
            transitions.append((ts, "appear" if present else "disappear"))
            prev = present

    disappearances = [t for t, kind in transitions if kind == "disappear"]
    present_frac = sum(1 for _, c in samples if c > 0) / max(1, len(samples))
    if verbose:
        for ts, kind in transitions:
            print(f"      {ts*1000:8.1f} ms  menu {kind}")
    return {
        "samples": len(samples),
        "seconds": round(seconds, 2),
        "menu_present_fraction": round(present_frac, 4),
        "disappearances": len(disappearances),
        "transitions": [{"at_ms": round(t * 1000, 1), "kind": k}
                        for t, k in transitions][:40],
        # Row-count churn: the popup resizing every frame also reads as flicker.
        "distinct_row_counts": sorted({c for _, c in samples}),
    }


def watch_static_rows(client: Screen, seconds: float) -> dict:
    """Count how often non-animation rows above the composer change.

    On a fresh idle session everything except the decorative animation rows is
    static, so any churn there is a rendering artifact the user perceives as the
    screen "jittering" while they type.
    """
    baseline = client.rows()
    changes: dict[int, int] = {}
    t0 = time.monotonic()
    prev = baseline
    while time.monotonic() - t0 < seconds:
        rows = client.rows()
        for idx, (a, b) in enumerate(zip(prev, rows)):
            if a != b:
                changes[idx] = changes.get(idx, 0) + 1
        prev = rows
        time.sleep(0.004)
    return {
        "changed_rows": len(changes),
        "busiest_rows": sorted(changes.items(), key=lambda kv: -kv[1])[:8],
        "total_row_changes": sum(changes.values()),
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
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--watch-s", type=float, default=3.0)
    ap.add_argument("--no-idle-animation", action="store_true",
                    help="A/B: disable the decorative animation for this run")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"binary not found: {binary}")
        return 3

    root = Path(tempfile.mkdtemp(prefix="jcode-slash-flicker-"))
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
    # The donut only runs in the Full performance tier. A busy build machine
    # would otherwise silently drop to Reduced and hide the bug.
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
        env["ANTHROPIC_API_KEY"] = "sk-ant-repro-slash-flicker"
    debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    if not args.json:
        print("== jcode slash-menu flicker repro ==")
        print(f"  binary : {binary}")
        print(f"  donut  : {'off' if args.no_idle_animation else 'on'}")

    server_log = root / "server.log"
    log_fh = server_log.open("wb")
    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=log_fh, stderr=subprocess.STDOUT, preexec_fn=os.setsid)

    client: Screen | None = None
    result: dict = {"binary": binary,
                    "idle_animation": not args.no_idle_animation}
    try:
        try:
            wait_for_socket(Path(env["JCODE_SOCKET"]))
            wait_for_socket(debug_sock)
        except RuntimeError:
            print("server never bound its sockets; log tail:")
            print(server_log.read_text()[-4000:])
            return 3

        session_id = dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        if session_id.startswith("{"):
            session_id = json.loads(session_id).get("session_id", "")
        session_id = session_id.split()[-1] if session_id else ""
        if not session_id:
            print("could not create a session")
            return 3

        client = launch(binary, env, session_id, cmd_path, resp_path)
        if not settle(cmd_path, resp_path):
            print("client never came up on the debug channel")
            return 3
        time.sleep(2.0)

        # Type `/` to open the slash-command popup, then watch the *screen*.
        client.send(b"/")
        time.sleep(0.35)
        rows = client.rows()
        # Discover the popup's own text from the live screen rather than
        # hard-coding command names, so this keeps working as commands change.
        candidates = ("/help", "/model", "/clear", "/resume", "/config")
        needles = tuple(c for c in candidates if any(c in r for r in rows))
        if not needles:
            result["error"] = "slash popup never appeared"
            result["screen"] = rows
            print(json.dumps(result, indent=2)[:4000])
            return 3
        result["menu_needles"] = list(needles)
        if not args.json:
            print(f"  popup  : detected via {needles}")
            print(f"  watching the rendered screen for {args.watch_s}s ...")

        menu = watch_menu(client, needles, args.watch_s, verbose=args.verbose)
        result["menu_watch"] = menu

        static = watch_static_rows(client, 1.5)
        result["static_rows_watch"] = static

        try:
            result["draw_stats"] = json.loads(
                client_cmd(cmd_path, resp_path, "draw-stats 16"))
        except Exception as e:  # noqa: BLE001
            result["draw_stats"] = {"error": str(e)}

        # The menu must never disappear once open. A single disappearance is
        # already user-visible blinking.
        flicker = menu["disappearances"] > 0
        result["flicker_reproduced"] = flicker

        if args.json:
            print(json.dumps(result, indent=2))
        else:
            print(f"\n  menu present {menu['menu_present_fraction']*100:.1f}% "
                  f"of samples, disappeared {menu['disappearances']}x")
            print(f"  transitions: {json.dumps(menu['transitions'][:12])}")
            print(f"  static-row churn: {json.dumps(static)}")
            ds = result.get("draw_stats") or {}
            if isinstance(ds, dict):
                print("  idle_animation:",
                      json.dumps(ds.get("idle_animation"), indent=2))
                summary = ds.get("summary") or {}
                print("  render_ms:", json.dumps(summary.get("render_ms")))
                print("  draws_per_second:", summary.get("draws_per_second"))
            if flicker:
                print(f"\n  FLICKER REPRODUCED: the slash menu was erased "
                      f"{menu['disappearances']}x while open")
            else:
                print("\n  no flicker: the menu stayed on screen")
        return 1 if flicker else 0
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
