#!/usr/bin/env python3
"""
End-to-end verifier for the Alt+Shift+E "expand edit diff" shortcut.

Why this exists
---------------
The handler logic for Alt+Shift+E is covered by unit tests, but those tests
inject *synthetic* `crossterm::KeyEvent`s. They never exercise:

  * crossterm's real terminal escape-sequence decoder,
  * the live event loop (local + remote client),
  * the kitty keyboard protocol bytes a real terminal actually sends.

So a unit test can be green while the live shortcut does nothing. This harness
closes that gap by driving the **real jcode binary** under a pseudo-terminal and
feeding it the **exact bytes** a terminal emits for Alt+Shift+E, then reading the
resulting diff state back over the debug socket.

What "pass" means (100% confidence)
-----------------------------------
The harness runs three checks against one live client:

  1. NEGATIVE CONTROL
     Confirm the fixture starts collapsed (diff_mode == Inline), so a later
     "FullInline" reading cannot be a false positive from a pre-expanded state.

  2. RAW-BYTES E2E (the real test)
     Write the literal Alt+Shift+E byte sequence (kitty CSI-u form, plus the
     legacy ESC-prefixed form) into the PTY master. crossterm decodes it, the
     live loop dispatches it, and we assert diff_mode flips to FullInline.

  3. POSITIVE CONTROL (decode-independent)
     Reset the fixture, then drive the *same* handler through the debug socket's
     synthetic `keys:alt+shift+e`. If raw bytes fail but synthetic keys succeed,
     the break is in terminal decode / byte delivery, not the handler.

Each byte encoding is tried against a freshly reset fixture so we learn exactly
which terminal encoding(s) work end to end.

Everything runs in a throwaway KCODE_HOME / runtime dir / socket. The user's
real server and sessions are never touched.

Usage
-----
  python3 scripts/repro_expand_edit_shortcut.py [--binary PATH] [--keep] [-v]

Exit codes:
  0 = raw-bytes Alt+Shift+E expands the diff end to end (shortcut works)
  2 = raw bytes do NOT expand, but synthetic keys do (decode/delivery break)
  3 = even synthetic keys fail (handler/fixture break) or setup failure
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
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


# ──────────────────────────────────────────────────────────────────────────────
# The exact bytes a terminal sends for Alt+Shift+E.
#
# Captured live from kitty 0.46 with the kitty keyboard protocol enabled (the
# protocol jcode turns on at startup):
#     press   = ESC [ 101 ; 4 u      -> b"\x1b[101;4u"
#     release = ESC [ 101 ; 4 : 3 u  -> b"\x1b[101;4:3u"
# (101 = 'e' codepoint, modifier mask 4 = (4-1)=3 = SHIFT|ALT.)
#
# We also include the legacy xterm encodings some terminals use when the kitty
# protocol is not negotiated, so the harness reports which encodings work.
# ──────────────────────────────────────────────────────────────────────────────
KEY_ENCODINGS: list[tuple[str, bytes]] = [
    # kitty keyboard protocol (what this machine's kitty actually sends)
    ("kitty-csi-u (e;SHIFT|ALT)", b"\x1b[101;4u\x1b[101;4:3u"),
    # kitty form some terminals send with the uppercase codepoint
    ("kitty-csi-u (E;SHIFT|ALT)", b"\x1b[69;4u\x1b[69;4:3u"),
    # legacy: ESC then Shift+E (Alt=ESC prefix, Shift folded into uppercase)
    ("legacy esc+E", b"\x1bE"),
    # legacy: ESC then lowercase e (some terminals drop the shift bit)
    ("legacy esc+e", b"\x1be"),
]


# ──────────────────────────────────────────────────────────────────────────────
# debug socket helpers (line-delimited JSON over a unix socket)
# ──────────────────────────────────────────────────────────────────────────────
def _recv_debug_response(sock: socket.socket, timeout: float) -> dict:
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


def dbg(debug_sock: Path, command: str, session_id: str | None = None,
        timeout: float = 30.0) -> dict:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(str(debug_sock))
    try:
        req = {"type": "debug_command", "id": 1, "command": command}
        if session_id:
            req["session_id"] = session_id
        s.sendall((json.dumps(req) + "\n").encode())
        return _recv_debug_response(s, timeout)
    finally:
        s.close()


def dbg_output(debug_sock: Path, command: str, session_id: str | None = None,
               timeout: float = 30.0) -> str:
    resp = dbg(debug_sock, command, session_id=session_id, timeout=timeout)
    if resp.get("type") == "error":
        raise RuntimeError(f"debug error for {command!r}: {resp.get('message')}")
    if resp.get("ok") is False:
        raise RuntimeError(f"command {command!r} failed: {resp.get('output')}")
    return resp.get("output", "")


def wait_for_socket(path: Path, timeout_s: float = 25.0) -> None:
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


# ──────────────────────────────────────────────────────────────────────────────
# A PTY-driven jcode client. This is the real live-render input path; bytes
# written to master_fd are decoded by crossterm exactly as a terminal would.
# ──────────────────────────────────────────────────────────────────────────────
_TERM_REPLIES = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[c", b"\x1b[?62;c"),
    (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
    (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
    (b"\x1b[14t", b"\x1b[4;600;800t"),
    (b"\x1b[16t", b"\x1b[6;16;8t"),
    (b"\x1b[18t", b"\x1b[8;24;80t"),
    (b"\x1b[?1016$p", b"\x1b[?1016;1$y"),
    (b"\x1b[?2027$p", b"\x1b[?2027;1$y"),
    (b"\x1b[?2031$p", b"\x1b[?2031;1$y"),
    (b"\x1b[?1004$p", b"\x1b[?1004;1$y"),
    (b"\x1b[?2004$p", b"\x1b[?2004;1$y"),
    (b"\x1b[?2026$p", b"\x1b[?2026;1$y"),
    # kitty keyboard protocol query: report DISAMBIGUATE|REPORT_EVENT_TYPES active
    (b"\x1b[?u", b"\x1b[?3u"),
]


@dataclass
class LiveClient:
    name: str
    proc: subprocess.Popen
    master_fd: int
    stop: threading.Event = field(default_factory=threading.Event)
    thread: threading.Thread | None = None
    total_bytes: int = 0

    def _pump(self) -> None:
        buffer = b""
        while not self.stop.is_set():
            try:
                rlist, _, _ = select.select([self.master_fd], [], [], 0.1)
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
            self.total_bytes += len(chunk)
            buffer = (buffer + chunk)[-8192:]
            changed = True
            while changed:
                changed = False
                for query, response in _TERM_REPLIES:
                    if query in buffer:
                        try:
                            os.write(self.master_fd, response)
                        except OSError:
                            pass
                        buffer = buffer.replace(query, b"")
                        changed = True

    def start_pump(self) -> None:
        self.thread = threading.Thread(target=self._pump, daemon=True)
        self.thread.start()

    def alive(self) -> bool:
        return self.proc.poll() is None

    def send_bytes(self, data: bytes) -> None:
        os.write(self.master_fd, data)

    def shutdown(self) -> None:
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=1.0)
        try:
            os.killpg(self.proc.pid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            self.proc.wait(timeout=2.0)
        except Exception:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            os.close(self.master_fd)
        except OSError:
            pass


def launch_client(binary: str, env: dict, session_id: str, name: str,
                  debug_cmd: Path, debug_resp: Path) -> LiveClient:
    master_fd, slave_fd = pty.openpty()
    cenv = dict(env)
    # Route this client's file-based debug channel to per-client paths so we can
    # talk to *this* live TUI directly (fixture setup + state readback + the
    # synthetic-key positive control).
    cenv["KCODE_DEBUG_CMD_PATH"] = str(debug_cmd)
    cenv["KCODE_DEBUG_RESPONSE_PATH"] = str(debug_resp)
    proc = subprocess.Popen(
        [
            binary,
            "--no-update",
            "--no-selfdev",
            "--socket", env["KCODE_SOCKET"],
            "--resume", session_id,
        ],
        stdin=slave_fd, stdout=slave_fd, stderr=slave_fd,
        env=cenv, preexec_fn=os.setsid,
    )
    os.close(slave_fd)
    os.set_blocking(master_fd, False)
    client = LiveClient(name=name, proc=proc, master_fd=master_fd)
    client.start_pump()
    return client


# ──────────────────────────────────────────────────────────────────────────────
# per-client file-based debug channel (talks to the live TUI, not the server)
# ──────────────────────────────────────────────────────────────────────────────
def client_cmd(debug_cmd: Path, debug_resp: Path, command: str,
               timeout_s: float = 6.0) -> str:
    """Send a command to a live TUI client via its file debug channel."""
    try:
        debug_resp.unlink()
    except FileNotFoundError:
        pass
    debug_cmd.write_text(command)
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if debug_resp.exists():
            # small settle so the write completes
            time.sleep(0.02)
            return debug_resp.read_text()
        time.sleep(0.02)
    raise RuntimeError(f"client did not answer debug command {command!r} in {timeout_s}s")


def client_diff_mode(debug_cmd: Path, debug_resp: Path) -> dict:
    raw = client_cmd(debug_cmd, debug_resp, "expand-badge-state")
    return json.loads(raw)


def reset_fixture(debug_cmd: Path, debug_resp: Path) -> dict:
    raw = client_cmd(debug_cmd, debug_resp, "expand-badge-fixture")
    return json.loads(raw)


# ──────────────────────────────────────────────────────────────────────────────
# minimal session seed (just needs to exist so the client can resume)
# ──────────────────────────────────────────────────────────────────────────────
def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S.000000000Z", time.gmtime())


def seed_session(home: Path, working_dir: str) -> str:
    sessions_dir = home / "sessions"
    sessions_dir.mkdir(parents=True, exist_ok=True)
    session_id = f"session_expand_{int(time.time()*1000)}_{uuid.uuid4().hex[:12]}"
    messages = [{
        "id": f"message_{uuid.uuid4().hex}",
        "role": "user",
        "content": [{"type": "text", "text": "seed for expand-edit shortcut test"}],
        "timestamp": now_iso(),
    }]
    session = {
        "id": session_id,
        "parent_id": None,
        "title": "expand-edit-shortcut",
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "working_dir": working_dir,
        "messages": messages,
    }
    (sessions_dir / f"{session_id}.json").write_text(json.dumps(session))
    return session_id


# ──────────────────────────────────────────────────────────────────────────────
def settle_client(debug_cmd: Path, debug_resp: Path, timeout_s: float = 20.0,
                  verbose: bool = False) -> bool:
    deadline = time.time() + timeout_s
    last_err = None
    while time.time() < deadline:
        try:
            client_cmd(debug_cmd, debug_resp, "expand-badge-state", timeout_s=2.0)
            return True
        except Exception as e:  # noqa: BLE001
            last_err = e
            time.sleep(0.2)
    if verbose and last_err:
        print(f"    (settle never answered: {last_err})")
    return False


def try_raw_encoding(client: LiveClient, debug_cmd: Path, debug_resp: Path,
                     label: str, raw: bytes, verbose: bool) -> bool:
    """Reset to collapsed, feed raw bytes, return True if it expanded."""
    reset_fixture(debug_cmd, debug_resp)
    time.sleep(0.15)
    before = client_diff_mode(debug_cmd, debug_resp)
    if before.get("diff_mode") != "Inline":
        # Force a clean collapsed baseline; never report a false positive.
        if verbose:
            print(f"    [{label}] baseline not Inline ({before.get('diff_mode')}), retrying reset")
        reset_fixture(debug_cmd, debug_resp)
        time.sleep(0.2)
        before = client_diff_mode(debug_cmd, debug_resp)
    client.send_bytes(raw)
    # Give the event loop time to read + dispatch the bytes.
    deadline = time.time() + 2.0
    after = before
    while time.time() < deadline:
        time.sleep(0.1)
        after = client_diff_mode(debug_cmd, debug_resp)
        if after.get("diff_mode") == "FullInline":
            break
    ok = (before.get("diff_mode") == "Inline"
          and after.get("diff_mode") == "FullInline")
    status = "✅ EXPANDED" if ok else "—  no change"
    print(f"    [{label:<28}] {before.get('diff_mode')} -> {after.get('diff_mode')}   {status}")
    return ok


def negative_discriminator(client: LiveClient, debug_cmd: Path, debug_resp: Path,
                           verbose: bool) -> bool:
    """Prove the detector can tell expand from no-expand.

    Feed a plain 'e' (no Alt/Shift). It must NOT expand the diff; otherwise a
    later "FullInline" reading would be meaningless. Returns True if the
    discriminator behaved correctly (stayed collapsed).
    """
    reset_fixture(debug_cmd, debug_resp)
    time.sleep(0.15)
    before = client_diff_mode(debug_cmd, debug_resp)
    client.send_bytes(b"e")          # plain ASCII e
    client.send_bytes(b"\x1b[101u")  # kitty CSI-u plain 'e', no modifiers
    time.sleep(0.6)
    after = client_diff_mode(debug_cmd, debug_resp)
    ok = (before.get("diff_mode") == "Inline"
          and after.get("diff_mode") == "Inline")
    status = "✅ stayed collapsed" if ok else "❌ unexpectedly expanded"
    print(f"    [{'plain e (no Alt/Shift)':<28}] "
          f"{before.get('diff_mode')} -> {after.get('diff_mode')}   {status}")
    return ok


def synthetic_positive_control(client: LiveClient, debug_cmd: Path, debug_resp: Path,
                               verbose: bool) -> bool:
    reset_fixture(debug_cmd, debug_resp)
    time.sleep(0.15)
    before = client_diff_mode(debug_cmd, debug_resp)
    client_cmd(debug_cmd, debug_resp, "keys:alt+shift+e")
    time.sleep(0.3)
    after = client_diff_mode(debug_cmd, debug_resp)
    ok = (before.get("diff_mode") == "Inline"
          and after.get("diff_mode") == "FullInline")
    status = "✅ EXPANDED" if ok else "—  no change"
    print(f"    [{'synthetic keys:alt+shift+e':<28}] "
          f"{before.get('diff_mode')} -> {after.get('diff_mode')}   {status}")
    return ok


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    default_bin = Path.home() / ".kcode" / "builds" / "current" / "jcode"
    ap.add_argument("--binary", default=str(default_bin))
    ap.add_argument("--keep", action="store_true", help="keep temp home on exit")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"❌ binary not found: {binary}")
        return 3

    root = Path(tempfile.mkdtemp(prefix="jcode-expand-edit-"))
    home = root / "home"
    run = root / "run"
    home.mkdir(parents=True, exist_ok=True)
    run.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["KCODE_HOME"] = str(home)
    env["KCODE_RUNTIME_DIR"] = str(run)
    env["KCODE_SOCKET"] = str(run / "kcode.sock")
    env["KCODE_NO_TELEMETRY"] = "1"
    env["KCODE_DEBUG_CONTROL"] = "1"
    env["KCODE_TEMP_SERVER"] = "1"
    env["KCODE_SERVER_OWNER_PID"] = str(os.getpid())
    debug_sock = run / "kcode-debug.sock"
    client_cmd_path = run / "client_debug_cmd"
    client_resp_path = run / "client_debug_resp"

    print("╔══════════════════════════════════════════════════════════╗")
    print("║  Alt+Shift+E expand-edit shortcut: end-to-end verifier   ║")
    print("╚══════════════════════════════════════════════════════════╝")
    print(f"  binary : {binary}")
    print(f"  home   : {home}")

    server = subprocess.Popen(
        [binary, "serve", "--socket", env["KCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        preexec_fn=os.setsid,
    )

    client: LiveClient | None = None
    rc = 3
    try:
        wait_for_socket(Path(env["KCODE_SOCKET"]))
        wait_for_socket(debug_sock)
        print("  server : up")

        session_id = seed_session(home, str(REPO_ROOT))
        print(f"  seeded : {session_id}")

        client = launch_client(binary, env, session_id, "client-A",
                               client_cmd_path, client_resp_path)
        if not settle_client(client_cmd_path, client_resp_path, verbose=args.verbose):
            print("  ❌ client never came up on the file debug channel")
            return 3
        print("  client : up (PTY)\n")

        # ── 1. negative control: fixture must start collapsed ────────────────
        print("── 1. negative control (fixture starts collapsed) ──────────")
        fx = reset_fixture(client_cmd_path, client_resp_path)
        time.sleep(0.2)
        state = client_diff_mode(client_cmd_path, client_resp_path)
        collapsed_ok = state.get("diff_mode") == "Inline"
        print(f"    fixture diff_mode = {state.get('diff_mode')}   "
              f"{'✅' if collapsed_ok else '❌ expected Inline'}")
        if not collapsed_ok:
            print("\n  ❌ could not establish a collapsed baseline; aborting.")
            return 3

        # ── 2. discriminator: prove the detector can tell expand from no-op ──
        print("\n── 2. discriminator (plain 'e' must NOT expand) ────────────")
        disc_ok = negative_discriminator(
            client, client_cmd_path, client_resp_path, args.verbose)
        if not disc_ok:
            print("\n  ❌ detector has no discriminating power; results meaningless.")
            return 3

        # ── 3. RAW-BYTES end-to-end (the real test) ──────────────────────────
        print("\n── 3. raw terminal bytes -> live decode -> handler ─────────")
        raw_results = {}
        for label, raw in KEY_ENCODINGS:
            raw_results[label] = try_raw_encoding(
                client, client_cmd_path, client_resp_path, label, raw, args.verbose)
        raw_any = any(raw_results.values())

        # ── 4. synthetic positive control ────────────────────────────────────
        print("\n── 4. positive control (synthetic key, bypasses decode) ────")
        synth_ok = synthetic_positive_control(
            client, client_cmd_path, client_resp_path, args.verbose)

        # ── verdict ──────────────────────────────────────────────────────────
        print("\n╭──────────────────────── verdict ─────────────────────────╮")
        if raw_any:
            working = [k for k, v in raw_results.items() if v]
            print("│ ✅ Alt+Shift+E EXPANDS the diff via real terminal bytes. │")
            print(f"│    working encodings: {', '.join(working)}")
            rc = 0
        elif synth_ok:
            print("│ ⚠ Raw terminal bytes do NOT expand the diff, but the     │")
            print("│   handler works via synthetic keys.                      │")
            print("│   => break is in terminal decode / byte delivery,        │")
            print("│      NOT the expand handler logic.                       │")
            rc = 2
        else:
            print("│ ❌ Neither raw bytes nor synthetic keys expand the diff.  │")
            print("│    => break is in the expand handler / fixture itself.   │")
            rc = 3
        print("╰──────────────────────────────────────────────────────────╯")
        return rc
    finally:
        if client:
            client.shutdown()
        try:
            os.killpg(server.pid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            server.wait(timeout=3.0)
        except Exception:
            try:
                os.killpg(server.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        if args.keep:
            print(f"\n  (kept temp dir: {root})")
        else:
            import shutil
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
