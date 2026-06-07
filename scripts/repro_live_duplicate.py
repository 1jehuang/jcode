#!/usr/bin/env python3
"""
Reproduction / detection harness for GitHub issue #314:
  "Live transcript duplicates assistant commentary and tool calls"

Goal
----
We have no confirmed repro of #314. This harness builds a *programmatic,
read-back* environment for the live-render layer so duplication can be
detected objectively instead of by eyeballing a transcript.

Strategy
--------
The bug is a divergence between two layers:
  * server conversation history   -> source of truth (one copy per message)
  * client `display_messages`     -> what is actually rendered live

So the detector simply diffs the two. Each seeded assistant/tool/tool-result
carries a UNIQUE marker token ("MARK_xxxx"). If a marker shows up in more
display_messages than it appears in the server's history, that is the bug,
caught with the exact trigger isolated.

Everything runs in a throwaway JCODE_HOME / runtime dir / socket, so the
user's real server and sessions are never touched.

The suspected triggers (issue "Possible trigger" + "Notes") are exercised in
stages:
  1. fresh-resume            single headed client resumes the seeded session
  2. second-attach           a second headed client attaches to the same session
  3. reload                  client triggers /reload (server reload recovery path)
  4. detach-reattach         first client is killed, a new client reattaches

After each stage every live client's display history is read back and diffed
against server truth.

Usage
-----
  python3 scripts/repro_live_duplicate.py [--binary PATH] [--keep] [-v]

Exit code 0 = no duplication detected, 2 = duplication reproduced.
"""
from __future__ import annotations

import argparse
import json
import os
import pty
import select
import shutil
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
# debug socket helpers
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
            t = resp.get("type")
            if t in ("ack", "pong"):
                continue
            return resp
    raise RuntimeError("debug socket closed without a response")


def dbg(debug_sock: Path, command: str, session_id: str | None = None,
        timeout: float = 30.0) -> dict:
    """Send one debug command, return parsed {ok, output, ...}."""
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


def wait_for_socket(path: Path, timeout_s: float = 20.0) -> None:
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
# PTY-driven headed TUI client (this is the real live-render path)
# ──────────────────────────────────────────────────────────────────────────────

# Terminal capability queries the TUI emits at startup; we answer them so the
# client finishes initializing under a pseudo-terminal.
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

    def send_keys(self, data: bytes) -> None:
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


def launch_client(binary: str, env: dict, session_id: str, name: str) -> LiveClient:
    master_fd, slave_fd = pty.openpty()
    proc = subprocess.Popen(
        [
            binary,
            "--no-update",
            "--no-selfdev",
            "--socket", env["JCODE_SOCKET"],
            "--resume", session_id,
        ],
        stdin=slave_fd, stdout=slave_fd, stderr=slave_fd,
        env=env, preexec_fn=os.setsid,
    )
    os.close(slave_fd)
    os.set_blocking(master_fd, False)
    client = LiveClient(name=name, proc=proc, master_fd=master_fd)
    client.start_pump()
    return client


# ──────────────────────────────────────────────────────────────────────────────
# session seeding
# ──────────────────────────────────────────────────────────────────────────────

@dataclass
class SeededSession:
    session_id: str
    markers: dict[str, int]   # marker -> expected occurrence count in truth


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S.000000000Z", time.gmtime())


def make_stored_message(role: str, blocks: list[dict],
                        display_role: str | None = None) -> dict:
    msg = {
        "id": f"message_{uuid.uuid4().hex}",
        "role": role,
        "content": blocks,
        "timestamp": now_iso(),
    }
    if display_role:
        msg["display_role"] = display_role
    return msg


def seed_session(home: Path, working_dir: str, turns: int = 3) -> SeededSession:
    """Write a session transcript with assistant commentary + tool calls.

    Every assistant text, tool_use input, and tool_result carries a unique
    MARK_xxxx token so duplication can be counted unambiguously.
    """
    sessions_dir = home / "sessions"
    sessions_dir.mkdir(parents=True, exist_ok=True)

    session_id = f"session_repro_{int(time.time()*1000)}_{uuid.uuid4().hex[:16]}"
    markers: dict[str, int] = {}

    def mark(tag: str) -> str:
        token = f"MARK_{tag}"
        markers[token] = 1
        return token

    messages: list[dict] = []
    # opening user turn
    messages.append(make_stored_message("user", [
        {"type": "text", "text": f"{mark('USER_OPEN')} please run the repro tasks"}
    ]))

    for i in range(turns):
        a_text = mark(f"ASSIST_{i:02d}")
        t_in = mark(f"TOOLIN_{i:02d}")
        t_out = mark(f"TOOLOUT_{i:02d}")
        tool_id = f"toolu_{uuid.uuid4().hex[:20]}"
        # assistant commentary + a tool call (mirrors the issue's pattern)
        messages.append(make_stored_message("assistant", [
            {"type": "text", "text": f"{a_text} I'll inspect step {i}."},
            {"type": "tool_use", "id": tool_id, "name": "bash",
             "input": {"command": f"echo {t_in}", "intent": f"repro step {i}"}},
        ]))
        # tool result comes back on the user turn
        messages.append(make_stored_message("user", [
            {"type": "tool_result", "tool_use_id": tool_id,
             "content": f"{t_out} step {i} ok"},
        ]))

    # final assistant summary
    messages.append(make_stored_message("assistant", [
        {"type": "text", "text": f"{mark('ASSIST_FINAL')} all steps complete"}
    ]))

    session = {
        "id": session_id,
        "parent_id": None,
        "title": "repro-314",
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "messages": messages,
        "provider_key": "openai",
        "model": "gpt-5.5",
        "reasoning_effort": "low",
        "is_canary": False,
        "testing_build": None,
        "working_dir": working_dir,
        "short_name": "repro",
        "status": "Active",
        "last_pid": None,
        "last_active_at": now_iso(),
        "is_debug": True,
        "saved": False,
        "env_snapshots": [],
    }

    path = sessions_dir / f"{session_id}.json"
    path.write_text(json.dumps(session, indent=2))
    return SeededSession(session_id=session_id, markers=markers)


# ──────────────────────────────────────────────────────────────────────────────
# detection
# ──────────────────────────────────────────────────────────────────────────────

def count_markers_in_display(history_json: str, markers: dict[str, int]) -> dict[str, int]:
    """Count how many display_messages contain each marker."""
    counts = {m: 0 for m in markers}
    try:
        msgs = json.loads(history_json)
    except json.JSONDecodeError:
        return counts
    for m in msgs:
        blob = json.dumps(m)
        for marker in markers:
            if marker in blob:
                counts[marker] += 1
    return counts


def count_markers_in_text(text: str, markers: dict[str, int]) -> dict[str, int]:
    return {m: text.count(m) for m in markers}


@dataclass
class StageResult:
    stage: str
    client: str
    display_count: int
    duplicates: dict[str, int]   # marker -> count, only where count > 1
    error: str | None = None


def diff_stage(stage: str, client_name: str, history_json: str,
               markers: dict[str, int]) -> StageResult:
    try:
        msgs = json.loads(history_json)
        n = len(msgs)
    except json.JSONDecodeError:
        return StageResult(stage, client_name, 0, {}, error="non-JSON history")
    counts = count_markers_in_display(history_json, markers)
    dups = {m: c for m, c in counts.items() if c > markers[m]}
    return StageResult(stage, client_name, n, dups)


# ──────────────────────────────────────────────────────────────────────────────
# orchestration
# ──────────────────────────────────────────────────────────────────────────────

def settle(debug_sock: Path, session_id: str, client_name_hint: str,
           verbose: bool, attempts: int = 60) -> bool:
    """Poll until a client for this session answers client:history."""
    for i in range(attempts):
        try:
            out = dbg_output(debug_sock, "client:history", session_id=session_id,
                             timeout=8.0)
            if out.strip().startswith("["):
                if verbose:
                    print(f"      [{client_name_hint}] client:history ready after {i} polls")
                return True
        except Exception as e:
            if verbose and i % 10 == 0:
                print(f"      [{client_name_hint}] waiting ({i}): {e}")
        time.sleep(0.5)
    return False


def read_client_history(debug_sock: Path, session_id: str) -> str:
    return dbg_output(debug_sock, "client:history", session_id=session_id, timeout=10.0)


def run_self_test(debug_sock: Path, session_id: str, verbose: bool) -> bool:
    """Positive control: force a real duplicate into a live client and confirm
    the detector flags it. Proves the instrument is not a no-op.

    Injecting the same assistant marker twice produces two display_messages
    (assistant role is never coalesced), so a working detector must report x2.
    """
    marker = f"MARK_SELFTEST_{uuid.uuid4().hex[:8]}"
    expected = {marker: 1}
    for _ in range(2):
        dbg_output(debug_sock, f"client:inject:assistant:{marker} injected duplicate",
                   session_id=session_id, timeout=10.0)
        time.sleep(0.3)
    hist = read_client_history(debug_sock, session_id)
    r = diff_stage("self-test", "control", hist, expected)
    if verbose:
        print(f"      self-test marker {marker}: display_count={r.display_count} "
              f"duplicates={r.duplicates}")
    # detector must have caught exactly the injected duplicate
    return r.duplicates.get(marker, 0) == 2


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    default_bin = Path.home() / ".jcode" / "builds" / "current" / "jcode"
    ap.add_argument("--binary", default=str(default_bin))
    ap.add_argument("--turns", type=int, default=3, help="tool-call turns to seed")
    ap.add_argument("--keep", action="store_true", help="keep temp home on exit")
    ap.add_argument("--self-test", action=argparse.BooleanOptionalAction, default=True,
                    help="run positive-control: force a real duplicate and confirm "
                         "the detector flags it (default: on)")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"❌ binary not found: {binary}")
        return 1

    root = Path(tempfile.mkdtemp(prefix="jcode-repro314-"))
    home = root / "home"
    run = root / "run"
    home.mkdir(parents=True, exist_ok=True)
    run.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["JCODE_HOME"] = str(home)
    env["JCODE_RUNTIME_DIR"] = str(run)
    env["JCODE_SOCKET"] = str(run / "jcode.sock")
    env["JCODE_NO_TELEMETRY"] = "1"
    env["JCODE_DEBUG_CONTROL"] = "1"
    env["JCODE_TEMP_SERVER"] = "1"
    env["JCODE_SERVER_OWNER_PID"] = str(os.getpid())
    debug_sock = run / "jcode-debug.sock"

    print("╔══════════════════════════════════════════════════════════╗")
    print("║  repro #314: live transcript duplicate detector          ║")
    print("╚══════════════════════════════════════════════════════════╝")
    print(f"  binary : {binary}")
    print(f"  home   : {home}")
    print(f"  socket : {env['JCODE_SOCKET']}")

    server = subprocess.Popen(
        [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
         "--no-update", "--no-selfdev"],
        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        preexec_fn=os.setsid,
    )

    clients: list[LiveClient] = []
    results: list[StageResult] = []

    try:
        wait_for_socket(Path(env["JCODE_SOCKET"]))
        wait_for_socket(debug_sock)
        print("  server : up")

        seeded = seed_session(home, str(REPO_ROOT), turns=args.turns)
        print(f"  seeded : {seeded.session_id}  ({len(seeded.markers)} markers, "
              f"{args.turns} tool turns)")

        # cross-check server truth (each marker should be present once)
        try:
            server_hist = dbg_output(debug_sock, "history",
                                     session_id=seeded.session_id, timeout=10.0)
        except Exception:
            server_hist = ""
        # ── stage 1: fresh resume ───────────────────────────────────────────
        print("\n── stage 1: fresh-resume ─────────────────────────────────")
        c1 = launch_client(binary, env, seeded.session_id, "client-A")
        clients.append(c1)
        if not settle(debug_sock, seeded.session_id, "client-A", args.verbose):
            print("  ⚠ client-A never answered client:history")
        else:
            r = diff_stage("fresh-resume", "client-A",
                           read_client_history(debug_sock, seeded.session_id),
                           seeded.markers)
            results.append(r)
            report_stage(r, args.verbose)

        # ── stage 2: second concurrent attach ───────────────────────────────
        print("\n── stage 2: second-attach ────────────────────────────────")
        c2 = launch_client(binary, env, seeded.session_id, "client-B")
        clients.append(c2)
        settle(debug_sock, seeded.session_id, "client-B", args.verbose)
        time.sleep(1.0)
        # both clients now attached to the same session; read whichever the
        # server routes to (most-recent connection) plus re-read after a beat
        for label in ("after-B-attach", "after-B-attach-2"):
            try:
                r = diff_stage(f"second-attach/{label}", "active",
                               read_client_history(debug_sock, seeded.session_id),
                               seeded.markers)
                results.append(r)
                report_stage(r, args.verbose)
            except Exception as e:
                print(f"  ⚠ read failed: {e}")
            time.sleep(0.8)

        # ── stage 3: reload ─────────────────────────────────────────────────
        print("\n── stage 3: reload ───────────────────────────────────────")
        try:
            dbg_output(debug_sock, "client:reload", session_id=seeded.session_id,
                       timeout=10.0)
            print("  /reload triggered on active client")
        except Exception as e:
            print(f"  ⚠ reload trigger failed: {e}")
        time.sleep(2.0)
        if settle(debug_sock, seeded.session_id, "post-reload", args.verbose):
            r = diff_stage("post-reload", "active",
                           read_client_history(debug_sock, seeded.session_id),
                           seeded.markers)
            results.append(r)
            report_stage(r, args.verbose)

        # ── stage 4: detach + reattach ──────────────────────────────────────
        print("\n── stage 4: detach-reattach ──────────────────────────────")
        c1.shutdown()
        c2.shutdown()
        time.sleep(1.0)
        c3 = launch_client(binary, env, seeded.session_id, "client-C")
        clients.append(c3)
        if settle(debug_sock, seeded.session_id, "client-C", args.verbose):
            for label in ("reattach", "reattach-2"):
                r = diff_stage(f"detach-reattach/{label}", "client-C",
                               read_client_history(debug_sock, seeded.session_id),
                               seeded.markers)
                results.append(r)
                report_stage(r, args.verbose)
                time.sleep(0.8)

        # ── positive control: prove the detector can catch a real duplicate ──
        self_test_ok: bool | None = None
        if args.self_test:
            print("\n── self-test: positive control ───────────────────────────")
            try:
                self_test_ok = run_self_test(debug_sock, seeded.session_id,
                                             args.verbose)
            except Exception as e:
                print(f"  ⚠ self-test failed to run: {e}")
                self_test_ok = False
            if self_test_ok:
                print("  🟢 detector caught the forced duplicate (instrument live)")
            else:
                print("  🔴 detector MISSED a forced duplicate -> results untrustworthy")

        # ── summary ─────────────────────────────────────────────────────────
        print("\n" + "─" * 60)
        if args.self_test and not self_test_ok:
            print("  ⛔ INSTRUMENT INVALID: positive control failed.")
            print("     A clean run cannot be trusted because the detector did not")
            print("     flag a known-injected duplicate. Investigate the harness.")
            return 3
        reproduced = [r for r in results if r.duplicates]
        if reproduced:
            print("  🔴 DUPLICATION REPRODUCED")
            for r in reproduced:
                print(f"    stage={r.stage} client={r.client} "
                      f"display_count={r.display_count}")
                for marker, cnt in sorted(r.duplicates.items()):
                    print(f"        {marker}: rendered x{cnt} (expected x1)")
            return 2
        else:
            print("  🟢 no duplication detected across all stages")
            print(f"     stages checked: {len(results)}")
            if server_hist:
                sc = count_markers_in_text(server_hist, seeded.markers)
                bad = {m: c for m, c in sc.items() if c > 2}
                print(f"     server history marker check: "
                      f"{'clean' if not bad else bad}")
            return 0

    finally:
        for c in clients:
            c.shutdown()
        try:
            os.killpg(server.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            server.wait(timeout=3)
        except Exception:
            try:
                os.killpg(server.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        if args.keep:
            print(f"\n  (kept temp home: {root})")
        else:
            shutil.rmtree(root, ignore_errors=True)


def report_stage(r: StageResult, verbose: bool) -> None:
    if r.error:
        print(f"  {r.stage:28s} {r.client:10s} ERROR: {r.error}")
        return
    status = "🔴 DUP" if r.duplicates else "🟢 ok "
    print(f"  {status} {r.stage:28s} {r.client:10s} display_messages={r.display_count}")
    if r.duplicates:
        for marker, cnt in sorted(r.duplicates.items()):
            print(f"          ↳ {marker} x{cnt}")


if __name__ == "__main__":
    raise SystemExit(main())
