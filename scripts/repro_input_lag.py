#!/usr/bin/env python3
"""
Reproduce and measure input-line lag in a freshly spawned jcode TUI.

Why this exists
---------------
Users report that a *newly spawned* jcode is laggy: the slash-command popup
stalls and each keystroke takes a visible moment to appear in the input line.
Unit tests render synthetically and never observe the real
"byte in -> repainted glyph out" latency of the live binary.

What it measures
----------------
For every keystroke we write raw bytes into a PTY master and then wait for the
*terminal output* to change (the repaint that carries the new input line). The
delay between write and first repaint byte, and between write and repaint
quiescence, is the latency the user actually feels.

Two scripted scenarios run against one freshly created session (not a resume,
so it reflects a brand-new spawn):

  plain   : typing "hello world" (no popup)
  slash   : typing "/model" (drives the slash-command suggestion popup)

Afterwards the client's own instrumentation (`slow-frames`, `draw-stats`) is
dumped so a slow scenario can be attributed to a render stage.

Usage
-----
  python3 scripts/repro_input_lag.py [--binary PATH] [--json] [-v]

Exit codes:
  0 = all scenarios within budget
  1 = a scenario exceeded the latency budget (lag reproduced)
  3 = setup failure
"""
from __future__ import annotations

import argparse
import json
import os
import pty
import select
import re
import signal
import socket
import statistics
import subprocess
import tempfile
import threading
import time
import fcntl
import struct
import termios
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Per-keystroke budget. 16ms would be one frame; anything above 80ms is
# perceptible lag, so that is where we call the bug reproduced.
BUDGET_P50_MS = 40.0
BUDGET_P95_MS = 120.0
# An idle TUI must not repaint continuously. Anything above a few repaints per
# second means every keystroke competes with an already-busy render loop, which
# is what users perceive as a laggy input line.
BUDGET_IDLE_CHUNKS_PER_S = 8.0

_TERM_REPLIES = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[c", b"\x1b[?62;c"),
    (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
    (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
    (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
    (b"\x1b[14t", b"\x1b[4;600;800t"),
    (b"\x1b[16t", b"\x1b[6;16;8t"),
    (b"\x1b[18t", b"\x1b[8;40;120t"),
    (b"\x1b[?1016$p", b"\x1b[?1016;1$y"),
    (b"\x1b[?2027$p", b"\x1b[?2027;1$y"),
    (b"\x1b[?2031$p", b"\x1b[?2031;1$y"),
    (b"\x1b[?1004$p", b"\x1b[?1004;1$y"),
    (b"\x1b[?2004$p", b"\x1b[?2004;1$y"),
    (b"\x1b[?2026$p", b"\x1b[?2026;1$y"),
    (b"\x1b[?u", b"\x1b[?3u"),
]


# ── debug socket helpers ─────────────────────────────────────────────────────
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


# ── per-client file debug channel ────────────────────────────────────────────
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


# ── live PTY client ─────────────────────────────────────────────────────────
@dataclass
class LiveClient:
    proc: subprocess.Popen
    master_fd: int
    stop: threading.Event = field(default_factory=threading.Event)
    thread: threading.Thread | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)
    # monotonic timestamps of every output chunk observed
    output_events: list[float] = field(default_factory=list)
    total_bytes: int = 0

    def _pump(self) -> None:
        buffer = b""
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

    def mark(self) -> int:
        with self.lock:
            return len(self.output_events)

    def next_event_after(self, index: int) -> float | None:
        with self.lock:
            if len(self.output_events) > index:
                return self.output_events[index]
        return None

    def last_event(self) -> float | None:
        with self.lock:
            return self.output_events[-1] if self.output_events else None

    def idle_probe(self, window_s: float = 2.0) -> dict:
        """Measure how much the client repaints while nobody is typing.

        An idle TUI should be nearly silent. Continuous idle repaints mean every
        keystroke lands behind an already-busy render loop, which is exactly what
        "the input line is laggy" feels like.
        """
        with self.lock:
            start_idx = len(self.output_events)
            start_bytes = self.total_bytes
        t0 = time.monotonic()
        time.sleep(window_s)
        with self.lock:
            events = len(self.output_events) - start_idx
            written = self.total_bytes - start_bytes
        elapsed = time.monotonic() - t0
        return {
            "window_s": round(elapsed, 3),
            "idle_output_chunks": events,
            "idle_chunks_per_s": round(events / elapsed, 2),
            "idle_bytes": written,
            "idle_bytes_per_s": round(written / elapsed, 1),
        }

    def send_bytes(self, data: bytes) -> None:
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


def launch_client(binary: str, env: dict, session_id: str,
                  cmd_path: Path, resp_path: Path,
                  rows: int = 48, cols: int = 160) -> LiveClient:
    master_fd, slave_fd = pty.openpty()
    # A default 24x80 PTY is smaller than a real terminal window, and layout
    # size decides whether the decorative donut fits. Size the PTY like a real
    # window so the measured render path matches what the user sees.
    fcntl.ioctl(slave_fd, termios.TIOCSWINSZ,
                struct.pack("HHHH", rows, cols, 0, 0))
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
    client = LiveClient(proc=proc, master_fd=master_fd)
    client.start_pump()
    return client


def settle(cmd_path: Path, resp_path: Path, timeout_s: float = 40.0) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            client_cmd(cmd_path, resp_path, "input", timeout_s=2.0)
            return True
        except Exception:
            time.sleep(0.2)
    return False


# ── measurement ─────────────────────────────────────────────────────────────
@dataclass
class Scenario:
    name: str
    text: str
    gap_s: float = 0.12
    # When true, do not wait for the UI to go quiet before each keystroke. This
    # is the realistic "typing at speed" case, where a slow frame from the
    # previous character queues up behind the next one.
    burst: bool = False


def measure(client: LiveClient, scenario: Scenario,
            timeout_s: float = 2.0, verbose: bool = False) -> dict:
    """Type each char and time write -> first repaint and -> repaint quiescence.

    First-byte latency can be a bare cursor move, so the number a user actually
    perceives is quiescence: when the client stops emitting output for this
    keystroke, i.e. the new input line is fully on screen.
    """
    latencies: list[float] = []
    settled: list[float] = []
    for ch in scenario.text:
        if not scenario.burst:
            # Let the UI go quiet so we attribute the next repaint to our
            # keystroke.
            quiet_until = time.monotonic() + scenario.gap_s
            while time.monotonic() < quiet_until:
                time.sleep(0.01)
        idx = client.mark()
        t0 = time.monotonic()
        client.send_bytes(ch.encode())
        deadline = t0 + timeout_s
        seen = None
        while time.monotonic() < deadline:
            seen = client.next_event_after(idx)
            if seen is not None:
                break
            time.sleep(0.001)
        if seen is None:
            latencies.append(timeout_s * 1000.0)
            settled.append(timeout_s * 1000.0)
            if verbose:
                print(f"      {ch!r}: NO REPAINT within {timeout_s}s")
            continue
        ms = (seen - t0) * 1000.0
        latencies.append(ms)
        # Follow the output until it stays quiet for 25ms (or we hit the
        # timeout): that is when this keystroke's repaint is done.
        last = seen
        while time.monotonic() < deadline:
            time.sleep(0.005)
            latest = client.last_event()
            if latest is not None and latest > last:
                last = latest
                continue
            if time.monotonic() - last > 0.025:
                break
        settle_ms = (last - t0) * 1000.0
        settled.append(settle_ms)
        if verbose:
            print(f"      {ch!r}: first={ms:7.1f} ms  settled={settle_ms:7.1f} ms")
    ordered = sorted(latencies)
    ordered_settled = sorted(settled)

    def pct(values: list[float], p: float) -> float:
        return values[max(0, int(len(values) * p) - 1)]

    return {
        "scenario": scenario.name,
        "text": scenario.text,
        "count": len(latencies),
        "p50_ms": round(statistics.median(ordered), 2),
        "p95_ms": round(pct(ordered, 0.95), 2),
        "max_ms": round(max(ordered), 2),
        "mean_ms": round(statistics.fmean(ordered), 2),
        "settled_p50_ms": round(statistics.median(ordered_settled), 2),
        "settled_p95_ms": round(pct(ordered_settled, 0.95), 2),
        "settled_max_ms": round(max(ordered_settled), 2),
        "samples_ms": [round(x, 2) for x in latencies],
        "settled_samples_ms": [round(x, 2) for x in settled],
    }


def slow_frame_summary(cmd_path: Path, resp_path: Path) -> dict:
    try:
        raw = client_cmd(cmd_path, resp_path, "slow-frames 32")
        payload = json.loads(raw)
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)}
    frames = payload.get("frames") or payload.get("slow_frames") or []
    if isinstance(frames, list) and frames:
        return {
            "slow_frame_count": len(frames),
            "worst": sorted(
                (
                    {
                        "total_ms": f.get("total_ms"),
                        "prepare_ms": f.get("prepare_ms"),
                        "draw_ms": f.get("draw_ms"),
                        "messages_ms": f.get("messages_ms"),
                        "input_event": f.get("input_event"),
                    }
                    for f in frames
                    if isinstance(f, dict)
                ),
                key=lambda f: f.get("total_ms") or 0.0,
                reverse=True,
            )[:5],
        }
    return {"slow_frame_count": 0, "raw_keys": list(payload)[:8]}


def draw_stats_summary(cmd_path: Path, resp_path: Path) -> dict:
    """Ask the client why it is repainting, so a busy idle loop is attributable."""
    try:
        payload = json.loads(client_cmd(cmd_path, resp_path, "draw-stats 16"))
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)}
    out = {"redraw_schedule": payload.get("redraw_schedule")}
    for key in ("summary", "idle_animation", "window_ms"):
        if key in payload:
            out[key] = payload[key]
    calls = payload.get("draw_calls") or payload.get("calls") or []
    if isinstance(calls, list) and calls:
        out["recent_draw_calls"] = calls[-6:]
        out["draw_call_count"] = len(calls)
    else:
        out["draw_stats_keys"] = list(payload)[:10]
    return out


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    default_bin = REPO_ROOT / "target" / "selfdev" / "jcode"
    if not default_bin.exists():
        default_bin = Path.home() / ".jcode" / "builds" / "current" / "jcode"
    ap.add_argument("--binary", default=str(default_bin))
    ap.add_argument("--json", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--repeat", type=int, default=1,
                    help="repeat each scenario N times (noise control)")
    ap.add_argument("--live", action="store_true",
                    help="measure against the user's real server/home instead of "
                         "a throwaway one (reproduces real-world spawn conditions)")
    ap.add_argument("--immediate", action="store_true",
                    help="start typing as soon as the client answers, without "
                         "letting startup work finish first")
    ap.add_argument("--background-clients", type=int, default=0,
                    help="spawn N extra live TUI clients first. Real users run "
                         "many jcode windows at once, and each one repaints on a "
                         "tick, so this reproduces the contention a lone client "
                         "never sees.")
    ap.add_argument("--no-idle-animation", action="store_true",
                    help="disable the decorative idle animation for this run. "
                         "Pair with a default run to A/B whether the donut is "
                         "what makes a fresh spawn feel laggy.")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"binary not found: {binary}")
        return 3

    root = Path(tempfile.mkdtemp(prefix="jcode-input-lag-"))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    if args.live:
        # Talk to whatever server the user already runs, with their real home,
        # config, sessions, and model catalog. This is the configuration the lag
        # report came from; the throwaway home is too clean to reproduce it.
        real_runtime = Path(env.get("JCODE_RUNTIME_DIR")
                            or f"/run/user/{os.getuid()}")
        env["JCODE_SOCKET"] = env.get("JCODE_SOCKET") or str(real_runtime / "jcode.sock")
        env["JCODE_DEBUG_CONTROL"] = "1"
        debug_sock = real_runtime / "jcode-debug.sock"
    else:
        env["JCODE_HOME"] = str(home)
        env["JCODE_RUNTIME_DIR"] = str(run)
        env["JCODE_SOCKET"] = str(run / "jcode.sock")
        env["JCODE_NO_TELEMETRY"] = "1"
        env["JCODE_DEBUG_CONTROL"] = "1"
        env["JCODE_TEMP_SERVER"] = "1"
        env["JCODE_SERVER_OWNER_PID"] = str(os.getpid())
        # The server refuses to boot without credentials. We never issue a real
        # request, so a dummy key keeps the throwaway home fully isolated.
        if not env.get("ANTHROPIC_API_KEY"):
            env["ANTHROPIC_API_KEY"] = "sk-ant-repro-input-lag"
        debug_sock = run / "jcode-debug.sock"
    # Pin the theme so the client never issues an OSC 11 background query.
    # The client consumes that reply from stdin itself; a harness that also
    # answers it races the client and the leftover bytes get decoded as
    # composer keystrokes (observed as `]11;rgb:...` text in the input line),
    # which silently invalidates every measurement taken afterwards.
    env["JCODE_THEME"] = "dark"
    if args.no_idle_animation:
        env["JCODE_IDLE_ANIMATION"] = "false"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    if not args.json:
        print("== jcode input-lag repro ==")
        print(f"  binary : {binary}")
        print(f"  mode   : {'live (real server/home)' if args.live else 'isolated'}")
        print(f"  socket : {env['JCODE_SOCKET']}")
        print(f"  dbgsock: {debug_sock}")

    server_log = root / "server.log"
    server = None
    if not args.live:
        server_log_fh = server_log.open("wb")
        server = subprocess.Popen(
            [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
             "--no-update", "--no-selfdev"],
            env=env, stdout=server_log_fh, stderr=subprocess.STDOUT,
            preexec_fn=os.setsid,
        )

    client: LiveClient | None = None
    result: dict = {"binary": binary}
    try:
        try:
            wait_for_socket(Path(env["JCODE_SOCKET"]))
        except RuntimeError:
            print("server never bound its socket; log tail:")
            try:
                print(server_log.read_text()[-4000:])
            except OSError:
                pass
            return 3
        wait_for_socket(debug_sock)
        session_id = dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
        # `create_session` may answer with a JSON blob or a bare id.
        if session_id.startswith("{"):
            session_id = json.loads(session_id).get("session_id", "")
        session_id = session_id.split()[-1] if session_id else ""
        if not session_id:
            print("could not create a session")
            return 3
        if not args.json:
            print(f"  session: {session_id}")

        background: list[LiveClient] = []
        for i in range(max(0, args.background_clients)):
            bg_session = dbg(debug_sock, f"create_session:{REPO_ROOT}").strip()
            bg_session = bg_session.split()[-1] if bg_session else ""
            if not bg_session:
                break
            bg_cmd = run / f"bg_cmd_{i}"
            bg_resp = run / f"bg_resp_{i}"
            background.append(
                launch_client(binary, env, bg_session, bg_cmd, bg_resp))
        if background and not args.json:
            print(f"  background: {len(background)} extra live clients")
        if background:
            time.sleep(3.0)

        client = launch_client(binary, env, session_id, cmd_path, resp_path)
        spawn_t0 = time.monotonic()
        if not settle(cmd_path, resp_path, timeout_s=60.0):
            print("client never came up on the debug channel")
            return 3
        first_answer_ms = (time.monotonic() - spawn_t0) * 1000.0
        result["client_first_debug_answer_ms"] = round(first_answer_ms, 1)
        if not args.json:
            print(f"  client : up (PTY) after {first_answer_ms:.0f}ms\n")
        if not args.immediate:
            # Let startup churn (model catalog, auth probes) drain so the steady
            # state is measured separately from the spawn burst.
            time.sleep(2.0)

        scenarios = [
            # idle probe runs first so the busy-loop signal is not polluted by
            # our own typing.
            Scenario("plain-typing", "hello world"),
            Scenario("slash-popup", "/model"),
            Scenario("plain-after-slash", " gpt"),
            Scenario("plain-burst", "the quick brown fox", burst=True),
            Scenario("slash-burst", "/model gpt", burst=True),
        ]
        idle = client.idle_probe(2.0)
        result["idle"] = idle
        result["idle_draw_stats"] = draw_stats_summary(cmd_path, resp_path)
        if not args.json:
            print(f"  idle repaints: {idle['idle_chunks_per_s']}/s, "
                  f"{idle['idle_bytes_per_s']} B/s\n")
            print("  idle draw attribution:",
                  json.dumps(result["idle_draw_stats"], indent=2)[:3000], "\n")

        runs = []
        for _ in range(max(1, args.repeat)):
            for sc in scenarios:
                if not args.json:
                    print(f"  -- {sc.name}: typing {sc.text!r}")
                m = measure(client, sc, verbose=args.verbose)
                if not args.json:
                    print(f"     p50={m['p50_ms']}ms p95={m['p95_ms']}ms "
                          f"max={m['max_ms']}ms")
                runs.append(m)
                # clear the input buffer between scenarios
                client_cmd(cmd_path, resp_path, "set_input:")
                time.sleep(0.2)
        result["runs"] = runs
        result["slow_frames"] = slow_frame_summary(cmd_path, resp_path)

        worst_p50 = max(r["p50_ms"] for r in runs)
        worst_p95 = max(r["p95_ms"] for r in runs)
        worst_settled_p50 = max(r["settled_p50_ms"] for r in runs)
        worst_settled_p95 = max(r["settled_p95_ms"] for r in runs)
        result["budget"] = {"p50_ms": BUDGET_P50_MS, "p95_ms": BUDGET_P95_MS}
        result["budget"]["idle_chunks_per_s"] = BUDGET_IDLE_CHUNKS_PER_S
        # `settled_*` is reported for diagnosis only: while the client repaints
        # on an idle tick it never truly goes quiet, so quiescence cannot be
        # attributed to one keystroke. The verdict uses first-byte latency plus
        # the idle repaint rate, both of which are attributable.
        idle_busy = idle["idle_chunks_per_s"] > BUDGET_IDLE_CHUNKS_PER_S
        reproduced = (worst_p50 > BUDGET_P50_MS
                      or worst_p95 > BUDGET_P95_MS
                      or idle_busy)
        result["lag_reproduced"] = reproduced
        result["idle_loop_busy"] = idle_busy

        if args.json:
            print(json.dumps(result, indent=2))
        else:
            print("\n  slow frames:", json.dumps(result["slow_frames"], indent=2))
            if reproduced:
                print(f"\n  LAG REPRODUCED: first-byte p50={worst_p50}ms "
                      f"p95={worst_p95}ms (budget {BUDGET_P50_MS}/{BUDGET_P95_MS}), "
                      f"idle repaints={idle['idle_chunks_per_s']}/s "
                      f"(budget {BUDGET_IDLE_CHUNKS_PER_S}/s)")
                if idle_busy:
                    print("    -> the render loop repaints while idle; keystrokes "
                          "queue behind it.")
            else:
                print(f"\n  within budget: first-byte p50={worst_p50}ms "
                      f"p95={worst_p95}ms, idle repaints="
                      f"{idle['idle_chunks_per_s']}/s")
            print(f"  (diagnostic, confounded by idle repaints: settled "
                  f"p50={worst_settled_p50}ms p95={worst_settled_p95}ms)")
        return 1 if reproduced else 0
    finally:
        for bg in locals().get("background", []) or []:
            bg.shutdown()
        if client:
            client.shutdown()
        for sig in (signal.SIGTERM, signal.SIGKILL) if server else ():
            try:
                os.killpg(server.pid, sig)
                server.wait(timeout=3.0)
                break
            except (ProcessLookupError, PermissionError):
                break
            except Exception:
                continue
        if args.keep:
            print(f"\n  (kept temp dir: {root})")
        else:
            import shutil
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
