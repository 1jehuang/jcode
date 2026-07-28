#!/usr/bin/env python3
"""
Reproduce the "typed character flickers in and out" bug in a live jcode TUI.

The symptom
-----------
Spawn a fresh jcode, press `/`, and the slash appears, vanishes, and reappears.
Something repaints the composer from a state that does not yet contain the
keystroke, so the character is briefly erased after it was already drawn.

Why the existing harnesses miss it
----------------------------------
`repro_input_lag.py` measures *timing* (how long until some output arrives) and
unit tests render single synthetic frames. Neither one reads the screen the way
a user does. A frame that correctly contains the `/`, followed by a frame that
has lost it, followed by one that has it again, looks perfectly healthy to a
latency probe: bytes arrived promptly every time.

What this does instead
----------------------
Runs the real binary under a PTY and feeds its output into a `pyte` terminal
emulator, i.e. an actual screen model. After each keystroke we sample the
composer line repeatedly and record the sequence of values it held. The bug is
then directly observable as a non-monotonic sequence: the typed prefix appears,
then a later sample has lost it.

This distinguishes three different things a timing probe conflates:

  * `flicker`   the character was shown, then un-shown (the reported bug)
  * `late`      the character took a long time to first appear
  * `clean`     the character appeared and stayed

Usage
-----
  python3 scripts/repro_input_flicker.py [--binary PATH] [--live] [-v]

Exit codes:
  0 = no flicker observed
  1 = flicker reproduced (a typed character was erased after being drawn)
  3 = setup failure
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
import pty
import re
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path

import pyte

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


def create_session(debug_sock: Path, working_dir: Path) -> str:
    """Create a headless session and return its id.

    `create_session` answers with a JSON object. Scraping the last whitespace
    token out of it silently yields garbage, which then shows up as "the client
    never started" rather than "we asked to resume a session that does not
    exist".
    """
    raw = dbg(debug_sock, f"create_session:{working_dir}").strip()
    if raw.startswith("{"):
        session_id = json.loads(raw).get("session_id", "")
    else:
        session_id = raw.split()[-1] if raw else ""
    if not session_id.startswith("session_"):
        raise RuntimeError(f"unexpected create_session response: {raw[:200]!r}")
    return session_id


@dataclass
class EmulatedClient:
    """A live jcode under a PTY, with its output rendered into a real screen.

    Reading a `pyte` screen instead of raw bytes is what makes the flicker
    visible: the bug is a *state regression on screen*, which byte-level probes
    cannot see.
    """

    proc: subprocess.Popen
    master_fd: int
    screen: pyte.Screen
    stream: pyte.Stream
    stop: threading.Event = field(default_factory=threading.Event)
    thread: threading.Thread | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)
    frames: int = 0
    # (monotonic_time, composer_text) for every change, recorded by the reader
    # thread itself.
    #
    # Sampling the screen from the main thread needed the same lock the reader
    # holds while feeding bytes, and rendering 48 rows under that lock starved
    # the reader. That inflated every "time until the character appeared" reading
    # into the hundreds of milliseconds: the harness was measuring its own
    # contention, not the app. Recording here means the timestamp is exactly when
    # the bytes were rendered.
    composer_history: list[tuple[float, str | None]] = field(default_factory=list)
    # A crash inside the reader thread would otherwise look exactly like "the
    # client produced no output", which sends debugging in the wrong direction.
    pump_error: str | None = None

    def _pump(self) -> None:
        try:
            self._pump_inner()
        except BaseException as e:  # noqa: BLE001
            import traceback
            self.pump_error = "".join(
                traceback.format_exception(type(e), e, e.__traceback__))

    def _pump_inner(self) -> None:
        replies = b""
        while not self.stop.is_set():
            try:
                rlist, _, _ = select.select([self.master_fd], [], [], 0.02)
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
            with self.lock:
                self.stream.feed(chunk.decode("utf-8", errors="replace"))
                self.frames += 1
                current = self._composer_text_locked()
                if not self.composer_history or self.composer_history[-1][1] != current:
                    self.composer_history.append((time.monotonic(), current))
            replies = (replies + chunk)[-8192:]
            changed = True
            while changed:
                changed = False
                for query, response in _TERM_REPLIES:
                    if query in replies:
                        try:
                            os.write(self.master_fd, response)
                        except OSError:
                            pass
                        replies = replies.replace(query, b"")
                        changed = True

    def start_pump(self) -> None:
        self.thread = threading.Thread(target=self._pump, daemon=True)
        self.thread.start()

    def send_bytes(self, data: bytes) -> None:
        os.write(self.master_fd, data)

    def lines(self) -> list[str]:
        with self.lock:
            return [self.screen.display[y].rstrip() for y in range(ROWS)]

    def _composer_text_locked(self) -> str | None:
        """Composer contents, assuming `self.lock` is already held."""
        for y in range(ROWS):
            m = COMPOSER_PROMPT.match(self.screen.display[y].rstrip())
            if m:
                return TRAILING_STATUS.sub("", m.group(2)).rstrip()
        return None

    def history_since(self, index: int) -> list[tuple[float, str | None]]:
        with self.lock:
            return self.composer_history[index:]

    def history_mark(self) -> int:
        with self.lock:
            return len(self.composer_history)

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
                  cmd_path: Path, resp_path: Path) -> EmulatedClient:
    master_fd, slave_fd = pty.openpty()
    fcntl.ioctl(slave_fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    cenv = dict(env)
    cenv["JCODE_DEBUG_CMD_PATH"] = str(cmd_path)
    cenv["JCODE_DEBUG_RESPONSE_PATH"] = str(resp_path)
    cenv["TERM"] = "xterm-256color"
    # Pin the theme so the client never issues an OSC 11 background query. Under
    # this harness the reply can land in stdin and be decoded as composer input,
    # which prepends garbage to everything typed and would mask the real signal.
    cenv.setdefault("JCODE_THEME", "dark")
    proc = subprocess.Popen(
        [binary, "--no-update", "--no-selfdev",
         "--socket", env["JCODE_SOCKET"], "--resume", session_id],
        stdin=slave_fd, stdout=slave_fd, stderr=slave_fd,
        env=cenv, preexec_fn=os.setsid,
    )
    os.close(slave_fd)
    os.set_blocking(master_fd, False)
    screen = pyte.Screen(COLS, ROWS)
    client = EmulatedClient(proc=proc, master_fd=master_fd,
                            screen=screen, stream=pyte.Stream(screen))
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


# The composer renders with a turn-numbered prompt, e.g. `1> `. Anchoring on that
# is what makes this detector trustworthy: matching the typed text anywhere on
# screen reports a `/` from the header's paths before anything is typed, and
# "whichever line changed" drifts because the whole layout reflows.
COMPOSER_PROMPT = re.compile(r"^\s*(\d+)>\s?(.*)$")

# The context/token readout is right-aligned onto the composer row and contains a
# `/` of its own (e.g. `3.1k/1.0M ...`). Left in place it makes the detector claim
# a slash is on screen before anything is typed, so it has to be removed rather
# than matched around.
TRAILING_STATUS = re.compile(r"\s{2,}\S+/\S+\s+[▱▰]*\s*\d+%\s*$")


def composer_text(client: EmulatedClient) -> str | None:
    """Text the user has typed into the composer, or `None` if it is off screen.

    Returns only the editable portion: the prompt marker and the right-aligned
    status readout are stripped so neither can be mistaken for typed input.
    """
    for line in client.lines():
        m = COMPOSER_PROMPT.match(line)
        if m:
            return TRAILING_STATUS.sub("", m.group(2)).rstrip()
    return None


def composer_shows(client: EmulatedClient, text: str) -> bool:
    """Whether the typed text is present in the composer.

    Substring rather than suffix: the status readout is right-aligned onto the
    same screen row, so the composer row legitimately has trailing content after
    the cursor. An earlier suffix-anchored version reported every keystroke as
    missing for exactly that reason.
    """
    current = composer_text(client)
    return current is not None and text in current


@dataclass
class KeystrokeObservation:
    typed: str
    # Every composer value the screen actually held after this keystroke.
    timeline: list[tuple[float, str | None]]
    first_seen_ms: float | None
    flickered: bool
    lost_after_ms: float | None
    # What the app itself thinks the input buffer holds. This separates "the
    # keystroke never reached the app" from "the app has it but the screen does
    # not show it", which are completely different bugs.
    app_input: str | None = None
    composer_on_screen: str | None = None
    # Cost of one trivial debug round-trip, i.e. how long the run loop took to
    # service a request that does no work. A slow value here means the loop itself
    # is not turning over promptly.
    loop_round_trip_ms: float | None = None


def observe_keystroke(client: EmulatedClient, ch: str, expect: str,
                      cmd_path: Path | None = None, resp_path: Path | None = None,
                      watch_s: float = 1.5, verbose: bool = False,
                      ) -> KeystrokeObservation:
    """Type one character, then read back every composer value the screen held.

    The timeline comes from the reader thread, so timestamps reflect when the
    client's bytes were rendered rather than when this loop happened to look.
    """
    mark = client.history_mark()
    t0 = time.monotonic()
    client.send_bytes(ch.encode())

    # Measure the debug channel's own round-trip cost once. Polling it to learn
    # "when did the app receive the key" is useless: the channel is a file
    # handshake serviced by the run loop, and it measured ~300ms per call, exactly
    # matching the "arrival" times it produced. That is the probe's latency, not
    # input delivery, so reporting it as arrival would indict the wrong subsystem.
    # It is still worth recording, because a run loop that needs 300ms to answer a
    # trivial request is itself evidence about responsiveness.
    probe_overhead_ms: float | None = None
    if cmd_path is not None and resp_path is not None:
        probe_start = time.monotonic()
        try:
            client_cmd(cmd_path, resp_path, "input", timeout_s=2.0)
            probe_overhead_ms = (time.monotonic() - probe_start) * 1000.0
        except Exception:
            probe_overhead_ms = None
    remaining = (t0 + watch_s) - time.monotonic()
    if remaining > 0:
        time.sleep(remaining)

    timeline = [
        (round((at - t0) * 1000.0, 1), value)
        for at, value in client.history_since(mark)
    ]

    first_seen_ms: float | None = None
    lost_after_ms: float | None = None
    flickered = False
    for at_ms, value in timeline:
        visible = value is not None and expect in value
        if visible and first_seen_ms is None:
            first_seen_ms = at_ms
        # A composer state that no longer contains the keystroke, after one that
        # did, is the reported bug: the character was drawn and then erased.
        elif first_seen_ms is not None and not visible and not flickered:
            flickered = True
            lost_after_ms = at_ms

    if verbose:
        steps = " -> ".join(f"{at}ms:{value!r}" for at, value in timeline) or "(no change)"
        probe = "?" if probe_overhead_ms is None else f"{probe_overhead_ms:.0f}ms"
        print(f"      typed {expect!r}: screen {steps}  (loop round-trip {probe})")

    app_input = None
    if cmd_path is not None and resp_path is not None:
        try:
            raw = client_cmd(cmd_path, resp_path, "input", timeout_s=3.0)
            app_input = raw.strip()
        except Exception as e:  # noqa: BLE001
            app_input = f"<unavailable: {e}>"

    return KeystrokeObservation(
        typed=expect,
        timeline=timeline,
        first_seen_ms=None if first_seen_ms is None else round(first_seen_ms, 1),
        flickered=flickered,
        lost_after_ms=None if lost_after_ms is None else round(lost_after_ms, 1),
        app_input=app_input,
        composer_on_screen=composer_text(client),
        loop_round_trip_ms=(None if probe_overhead_ms is None
                            else round(probe_overhead_ms, 1)),
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    default_bin = REPO_ROOT / "target" / "selfdev" / "jcode"
    if not default_bin.exists():
        default_bin = Path.home() / ".jcode" / "builds" / "current" / "jcode"
    ap.add_argument("--binary", default=str(default_bin))
    ap.add_argument("--live", action="store_true",
                    help="use the user's real server/home instead of a throwaway "
                         "one, which is where the bug was reported")
    ap.add_argument("--repeat", type=int, default=3,
                    help="repeat the scenario N times; the flicker is racy, so a "
                         "single clean run proves nothing")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--dump-screen", action="store_true",
                    help="type '/' once and dump the before/after screens, so the "
                         "detector can be built against real output instead of a "
                         "guess about where the composer is")
    args = ap.parse_args()

    binary = str(Path(args.binary).resolve())
    if not Path(binary).exists():
        print(f"binary not found: {binary}")
        return 3

    root = Path(tempfile.mkdtemp(prefix="jcode-input-flicker-"))
    home, run = root / "home", root / "run"
    home.mkdir(parents=True)
    run.mkdir(parents=True)

    env = os.environ.copy()
    if args.live:
        real_runtime = Path(env.get("JCODE_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
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
        if not env.get("ANTHROPIC_API_KEY"):
            env["ANTHROPIC_API_KEY"] = "sk-ant-repro-input-flicker"
        debug_sock = run / "jcode-debug.sock"
    cmd_path, resp_path = run / "client_cmd", run / "client_resp"

    if not args.json:
        print("== jcode input-flicker repro ==")
        print(f"  binary : {binary}")
        print(f"  mode   : {'live' if args.live else 'isolated'}")

    server = None
    server_log = root / "server.log"
    if not args.live:
        server = subprocess.Popen(
            [binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket",
             "--no-update", "--no-selfdev"],
            env=env, stdout=server_log.open("wb"), stderr=subprocess.STDOUT,
            preexec_fn=os.setsid,
        )

    result: dict = {"binary": binary, "runs": []}
    clients: list[EmulatedClient] = []
    try:
        wait_for_socket(Path(env["JCODE_SOCKET"]))
        wait_for_socket(debug_sock)

        for run_idx in range(max(1, args.repeat)):
            session_id = create_session(debug_sock, REPO_ROOT)

            # A *fresh spawn* is the reported condition, so each repetition gets a
            # brand new client rather than reusing a warmed-up one.
            rc_cmd = run / f"cmd_{run_idx}"
            rc_resp = run / f"resp_{run_idx}"
            client = launch_client(binary, env, session_id, rc_cmd, rc_resp)
            clients.append(client)
            if not settle(rc_cmd, rc_resp):
                # The emulated screen is the best evidence available here: it
                # shows whether the client crashed, is stuck on a prompt, or
                # simply never wired up its debug channel.
                print("client never came up on the debug channel; screen was:")
                for line in client.lines():
                    if line:
                        print(f"    | {line}")
                print(f"    (process alive: {client.proc.poll() is None}, "
                      f"output chunks: {client.frames})")
                if client.pump_error:
                    print("    reader thread crashed:")
                    for line in client.pump_error.splitlines():
                        print(f"    ! {line}")
                return 3

            if not args.json:
                print(f"\n  -- run {run_idx + 1}: fresh spawn, typing '/model'")

            observations = []
            typed = ""
            # Snapshot the untouched screen so the composer can be identified by
            # what changes, rather than by matching text that also appears in the
            # header and transcript.
            baseline = client.lines()

            # Negative control. If the detector reports the text as visible
            if args.dump_screen:
                print("  --- screen before typing ---")
                for y, line in enumerate(baseline):
                    if line:
                        print(f"  {y:3} | {line}")
                client.send_bytes(b"/")
                time.sleep(0.6)
                after = client.lines()
                print("  --- screen after typing '/' (* = changed) ---")
                for y, line in enumerate(after):
                    if line:
                        changed = y >= len(baseline) or line != baseline[y]
                        print(f"  {y:3}{'*' if changed else ' '}| {line}")
                client.shutdown()
                return 0

            # Negative control. If the detector reports the text as visible
            # *before* it is typed, then a later "clean" verdict means nothing.
            # This is what caught an earlier version of this harness matching the
            # `/` inside the header's path.
            if composer_shows(client, "/"):
                print("  detector has no discriminating power: it sees '/' "
                      "before anything was typed. Screen:")
                for line in client.lines():
                    if line:
                        print(f"    | {line}")
                return 3

            for ch in "/model":
                typed += ch
                obs = observe_keystroke(client, ch, typed, rc_cmd, rc_resp,
                                        verbose=args.verbose)
                observations.append(obs)

            run_result = {
                "run": run_idx + 1,
                "session_id": session_id,
                "keystrokes": [
                    {
                        "typed": o.typed,
                        "first_seen_ms": o.first_seen_ms,
                        "flickered": o.flickered,
                        "lost_after_ms": o.lost_after_ms,
                    }
                    for o in observations
                ],
                "flicker_count": sum(1 for o in observations if o.flickered),
                "never_appeared": [o.typed for o in observations
                                   if o.first_seen_ms is None],
            }
            result["runs"].append(run_result)

            if not args.json:
                for o in observations:
                    if o.flickered:
                        print(f"     {o.typed!r}: FLICKER "
                              f"(shown at {o.first_seen_ms}ms, "
                              f"erased at {o.lost_after_ms}ms)")
                    elif o.first_seen_ms is None:
                        print(f"     {o.typed!r}: NEVER APPEARED")
                    else:
                        print(f"     {o.typed!r}: ok (shown at {o.first_seen_ms}ms)")
                    print(f"        loop round-trip={o.loop_round_trip_ms}ms  "
                          f"screen={o.composer_on_screen!r}")

            # Ask this client why its frames were slow *before* stopping it, so a
            # long paint delay is attributed to a render stage instead of guessed.
            try:
                payload = json.loads(
                    client_cmd(rc_cmd, rc_resp, "draw-stats 16", timeout_s=5.0))
                run_result["draw_summary"] = payload.get("summary")
                run_result["redraw_schedule"] = payload.get("redraw_schedule")
                if not args.json:
                    print(f"     draw: {json.dumps(payload.get('summary'))}")
                    sched = payload.get("redraw_schedule") or {}
                    print(f"     sched: interval={sched.get('interval_ms')}ms "
                          f"reason={sched.get('current_full_frame_reason')} "
                          f"anim={sched.get('idle_animation_active')}")
            except Exception as e:  # noqa: BLE001
                if not args.json:
                    print(f"     (draw-stats unavailable: {e})")

            client.shutdown()

        total_flicker = sum(r["flicker_count"] for r in result["runs"])
        never = [t for r in result["runs"] for t in r["never_appeared"]]
        result["total_flicker"] = total_flicker
        result["flicker_reproduced"] = total_flicker > 0

        if args.json:
            print(json.dumps(result, indent=2))
        elif total_flicker:
            print(f"\n  FLICKER REPRODUCED: {total_flicker} keystroke(s) were "
                  f"drawn and then erased")
        elif never:
            print(f"\n  no flicker, but these never appeared: {never}")
        else:
            print("\n  clean: every keystroke appeared and stayed")
        return 1 if total_flicker else 0
    finally:
        for c in clients:
            c.shutdown()
        if server:
            for sig in (signal.SIGTERM, signal.SIGKILL):
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
