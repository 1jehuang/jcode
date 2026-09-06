#!/usr/bin/env python3
"""Opt-in native SSH acceptance against a built CLI and an explicitly chosen host.

Required environment (no network access when unset):
  JCODE_NATIVE_SSH_BINARY=/absolute/path/to/local/jcode
  JCODE_NATIVE_SSH_HOST=jcode-dev
  JCODE_NATIVE_SSH_REMOTE_BINARY=/absolute/path/to/remote-wrapper
  JCODE_NATIVE_SSH_CWD=/absolute/remote/workspace
Optional JCODE_NATIVE_SSH_SERVER_SOCKET selects a prestarted isolated daemon.
The remote wrapper should select an isolated JCODE_HOME/JCODE_RUNTIME_DIR and a
fresh matching binary. Host keys must already be verified in system known_hosts.

Run: python3 tests/test_native_ssh_cli.py
Offline harness checks: python3 tests/test_native_ssh_cli.py --self-test

Creates one uniquely marked, context-only remote session and leaves it there as
an acceptance artifact. NEVER sends a model-turn message, installs software,
changes SSH configuration, or stops the remote daemon. Local Jcode state is
isolated, but system SSH keys/config/agent remain available. Linux /proc is used
to verify owned SSH children and private adapter sockets disappear on TUI quit.
"""

import contextlib
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import selectors
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
import uuid

PREFIX = "JCODE_NATIVE_SSH_"
MAX_FRAME = 8 * 1024 * 1024
MAX_STDERR = 16 * 1024
TIMEOUT = 60
SSH_FLAGS = [
    "-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
    "-o", "ForwardAgent=no", "-o", "ClearAllForwardings=yes",
    "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2",
    "-o", "PermitLocalCommand=no", "-o", "ForkAfterAuthentication=no",
    "-o", "StdinNull=no", "-o", "RemoteCommand=none", "-o", "SessionType=default",
    "-o", "ControlMaster=no", "-S", "none", "-o", "ConnectTimeout=30",
]
ANSI = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-_]")


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def visible(data):
    return ANSI.sub("", data.decode("utf-8", errors="replace"))


def configured():
    required = ("BINARY", "HOST", "REMOTE_BINARY", "CWD")
    values = {name: os.environ.get(PREFIX + name) for name in required}
    if not any(values.values()):
        print("SKIP native SSH acceptance: set " + ", ".join(PREFIX + n for n in required))
        return None
    missing = [PREFIX + name for name, value in values.items() if not value]
    require(not missing, "Incomplete opt-in configuration: " + ", ".join(missing))
    host = values["HOST"]
    require(bool(re.fullmatch(r"(?:[A-Za-z0-9_][A-Za-z0-9_.-]*@)?[A-Za-z0-9_\[:][A-Za-z0-9_.:\[\]%-]*", host)), "Invalid SSH host/alias")
    for name in ("REMOTE_BINARY", "CWD"):
        require(not any(ord(c) < 32 or ord(c) == 127 for c in values[name]), f"Control character in {name}")
    require(not values["REMOTE_BINARY"].startswith("-"), "Remote binary cannot be an option")
    values["BINARY"] = str(Path(values["BINARY"]).resolve(strict=True))
    require(os.access(values["BINARY"], os.X_OK), "Local binary is not executable")
    values["SERVER_SOCKET"] = os.environ.get(PREFIX + "SERVER_SOCKET")
    return values


def remote_command(config):
    argv = [config["REMOTE_BINARY"], "--no-update", "--no-selfdev", "--cwd", config["CWD"]]
    if config.get("SERVER_SOCKET"):
        argv += ["--socket", config["SERVER_SOCKET"]]
    argv += ["server", "stdio"]
    return 'PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; export PATH; exec ' + shlex.join(argv)


def local_command(config, *, cwd=None, tail=()):
    argv = [config["BINARY"], "--no-update", "--no-selfdev", "--ssh", config["HOST"],
            "--ssh-binary", config["REMOTE_BINARY"], "--remote-working-dir", cwd or config["CWD"]]
    if config.get("SERVER_SOCKET"):
        argv += ["--ssh-server-socket", config["SERVER_SOCKET"]]
    return argv + list(tail)


class Bridge:
    def __init__(self, config):
        self.process = subprocess.Popen(
            ["ssh", *SSH_FLAGS, "--", config["HOST"], remote_command(config)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=True, bufsize=0,
        )
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ, "stdout")
        self.selector.register(self.process.stderr, selectors.EVENT_READ, "stderr")
        self.output = bytearray()
        self.stderr = bytearray()
        self.events = []
        self.stdout_closed = False

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.process.stdin.close()
        self.process.stdin = None
        try:
            _, stderr = self.process.communicate(timeout=5)
            self.stderr.extend(stderr)
            del self.stderr[:-MAX_STDERR]
        except subprocess.TimeoutExpired:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(self.process.pid, signal.SIGKILL)
            self.process.communicate(timeout=5)
            if kind is None:
                raise AssertionError("Remote stdio bridge failed to exit on stdin EOF")
        finally:
            self.selector.close()
            self.process.stdout.close()
            self.process.stderr.close()
        if kind is None:
            require(self.process.returncode == 0,
                    f"SSH bridge exited {self.process.returncode}: {self.stderr.decode(errors='replace')}")

    def send(self, frame):
        # This harness must never accidentally start provider inference.
        if frame.get("type") == "message":
            require(frame.get("no_reply") is True, "Only context-only messages are permitted")
        self.process.stdin.write(json.dumps(frame).encode() + b"\n")
        self.process.stdin.flush()

    def frame(self, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while True:
            if b"\n" in self.output:
                raw, _, rest = self.output.partition(b"\n")
                self.output = bytearray(rest)
                require(len(raw) <= MAX_FRAME, "Oversized native event")
                frame = json.loads(raw)
                require(isinstance(frame, dict), "Native event must be an object")
                self.events.append(frame)
                return frame
            require(len(self.output) <= MAX_FRAME, "Unbounded native event")
            if self.stdout_closed or time.monotonic() >= deadline:
                raise AssertionError(
                    f"No native frame (exit={self.process.poll()}): "
                    + self.stderr.decode(errors="replace")
                )
            for key, _ in self.selector.select(min(0.2, max(0, deadline - time.monotonic()))):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    self.selector.unregister(key.fileobj)
                    if key.data == "stdout":
                        self.stdout_closed = True
                    continue
                if key.data == "stdout":
                    self.output.extend(chunk)
                else:
                    self.stderr.extend(chunk)
                    del self.stderr[:-MAX_STDERR]

    def until(self, predicate, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            frame = self.frame(deadline - time.monotonic())
            require(frame.get("type") != "error", f"Native server error: {frame}")
            require(frame.get("type") not in {"text_delta", "tool_start", "tool_exec", "tool_done", "thinking_delta"},
                    "Context-only acceptance unexpectedly observed model/tool activity")
            if predicate(frame):
                return frame
        raise AssertionError("Expected native event did not arrive")

    def handshake(self):
        header = self.frame()
        require(header.get("kind") == "jcode-native-stdio" and header.get("protocol") == 1,
                f"Wrong native SSH handshake: {header}")
        require(bool(header.get("version") and header.get("socket_path") and header.get("working_dir")),
                "Missing handshake identity metadata")
        self.send({"type": "ping", "id": 100})
        pong = self.until(lambda event: event.get("type") == "pong" and event.get("id") == 100)
        require(pong.get("native_ssh_protocol") == 1, "Daemon lacks native SSH persistence capability")
        return header

    def subscribe(self, cwd, instance, session_id=None):
        request = {
            "type": "subscribe", "id": 101, "working_dir": cwd,
            "client_instance_id": instance, "client_has_local_history": False,
            "allow_session_takeover": True, "continue_on_disconnect": True,
            "crash_on_disconnect": False, "terminal_env": [],
        }
        if session_id:
            request["target_session_id"] = session_id
        self.send(request)
        self.send({"type": "get_history", "id": 102})
        return self.until(lambda event: event.get("type") == "history")


def history_contains(history, sentinel):
    return any(sentinel in json.dumps(message) for message in history.get("messages", []))


def assert_no_local_transcript(home, session_id, sentinel):
    sessions = home / "sessions"
    if sessions.exists():
        for path in sessions.rglob("*"):
            if not path.is_file():
                continue
            require(session_id not in path.name, f"Remote session leaked to local transcript: {path}")
            require(sentinel.encode() not in path.read_bytes(), f"Remote context leaked to local transcript: {path}")


def process_info(pid):
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        fields = stat[stat.rfind(")") + 2:].split()
        command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        return int(fields[1]), fields[19], command
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return None


def owned_ssh(cli_pid):
    processes = {}
    for path in Path("/proc").iterdir():
        if path.name.isdigit():
            info = process_info(int(path.name))
            if info:
                processes[int(path.name)] = info
    descendants = {cli_pid}
    while True:
        expanded = descendants | {pid for pid, info in processes.items() if info[0] in descendants}
        if expanded == descendants:
            break
        descendants = expanded
    return {(pid, processes[pid][1]) for pid in descendants if pid in processes
            and processes[pid][2] and Path(os.fsdecode(processes[pid][2][0])).name == "ssh"}


def owned_sockets(pid):
    inodes = set()
    try:
        for fd in Path(f"/proc/{pid}/fd").iterdir():
            with contextlib.suppress(OSError):
                link = os.readlink(fd)
                if link.startswith("socket:["):
                    inodes.add(link[8:-1])
        return {Path(parts[7]) for line in Path("/proc/net/unix").read_text().splitlines()[1:]
                if len(parts := line.split()) >= 8 and parts[6] in inodes and "/jcode-ssh-" in parts[7]}
    except (FileNotFoundError, ProcessLookupError):
        return set()


def child_terminal():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


def tui_acceptance(config, env, local_cwd, session_id, sentinel):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 180, 0, 0))
    process = subprocess.Popen(local_command(config, tail=("--resume", session_id)),
                               stdin=slave, stdout=slave, stderr=slave, env=env,
                               cwd=local_cwd, preexec_fn=child_terminal, close_fds=True)
    os.close(slave)
    selector = selectors.DefaultSelector()
    selector.register(master, selectors.EVENT_READ)
    output = bytearray()
    ssh_children = set()
    sockets = set()
    answered = set()

    def pump(duration):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            ssh_children.update(owned_ssh(process.pid))
            sockets.update(owned_sockets(process.pid))
            for _, _ in selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                try:
                    chunk = os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        return
                    raise
                if not chunk:
                    return
                output.extend(chunk)
                require(len(output) <= 32 * 1024 * 1024, "Unbounded TUI output")
                # A minimal real terminal's replies, not application event mocks.
                for query, reply in [(b"\x1b[6n", b"\x1b[1;1R"), (b"\x1b[?u", b"\x1b[?0u"),
                                     (b"\x1b[c", b"\x1b[?1;2c"), (b"\x1b[>c", b"\x1b[>0;0;0c")]:
                    for match in re.finditer(re.escape(query), output):
                        token = (query, match.start())
                        if token not in answered:
                            os.write(master, reply)
                            answered.add(token)

    try:
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            pump(0.2)
            text = visible(output)
            if f"SSH {config['HOST']}" in text and sentinel in text:
                break
            require(process.poll() is None, f"TUI exited before showing remote history:\n{text[-6000:]}")
        else:
            raise AssertionError("TUI did not show SSH host and remote sentinel:\n" + visible(output)[-6000:])
        pump(0.5)
        text = visible(output).lower()
        for marker in ("welcome to jcode", "choose your provider", "let's get you set up", "sign in to get started"):
            require(marker not in text, f"Unexpected local onboarding: {marker}")
        require(ssh_children, "Did not observe a real owned SSH child")
        require(sockets, "Did not observe the private native adapter socket")
        for path in sockets:
            require(path.parent.stat().st_mode & 0o777 == 0o700,
                    f"Native adapter directory is not private0700: {path.parent}")
        os.write(master, b"/quit\r")
        deadline = time.monotonic() + 15
        while process.poll() is None and time.monotonic() < deadline:
            pump(0.1)
        require(process.poll() == 0, f"TUI did not quit successfully:\n{visible(output)[-6000:]}")
        for pid, start_time in ssh_children:
            info = process_info(pid)
            require(info is None or info[1] != start_time, f"Owned SSH child {pid} remains after TUI exit")
        require(all(not path.exists() for path in sockets), f"Private adapter sockets survived exit: {sockets}")
        print(f"PASS local PTY: SSH host + remote context, no onboarding, /quit reaped {len(ssh_children)} SSH child(ren)")
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=7)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
        # Failure cleanup may kill only exact recorded descendants, never other SSH.
        for pid, start_time in ssh_children:
            info = process_info(pid)
            if info and info[1] == start_time:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGKILL)
        selector.close()
        os.close(master)


def run_acceptance(config):
    require(sys.platform.startswith("linux"), "PTY owned-child acceptance requires Linux /proc")
    with tempfile.TemporaryDirectory(prefix="jcode-native-ssh-", dir=os.environ.get("JCODE_SCRATCH_DIR")) as root:
        root = Path(root)
        home = root / "jcode"
        runtime = root / "runtime"
        home.mkdir(mode=0o700)
        runtime.mkdir(mode=0o700)
        # Keep the user's real HOME only for explicitly requested system SSH
        # identity/config. Isolate all Jcode state and disable local UI hooks.
        env = {key: value for key, value in os.environ.items() if not key.startswith("JCODE_")}
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "KITTY_LISTEN_ON", "TMUX", "ZELLIJ"):
            env.pop(key, None)
        env.update(JCODE_HOME=str(home), JCODE_RUNTIME_DIR=str(runtime), XDG_RUNTIME_DIR=str(runtime),
                   JCODE_NO_TELEMETRY="1", JCODE_WAKE_MODE="external", TERM="xterm-256color",
                   DO_NOT_TRACK="1", NO_COLOR="0")
        # SSH agent may live under the original XDG runtime, but its absolute
        # SSH_AUTH_SOCK value is deliberately retained above.
        sentinel = "JCODE_SSH_CONTEXT_" + uuid.uuid4().hex
        instance = "native-ssh-acceptance-" + uuid.uuid4().hex
        pipeline = subprocess.run(
            ["ssh", *SSH_FLAGS, "--", config["HOST"], remote_command(config)],
            input=b'{"type":"ping","id":99}\n', capture_output=True, timeout=TIMEOUT,
        )
        require(pipeline.returncode == 0,
                "SSH pipeline failed on stdin EOF: " + pipeline.stderr.decode(errors="replace"))
        frames = [json.loads(line) for line in pipeline.stdout.splitlines() if line.strip()]
        require(frames and frames[0].get("kind") == "jcode-native-stdio", "Pipeline handshake missing")
        require(any(frame.get("type") == "pong" and frame.get("id") == 99 for frame in frames),
                "Pipeline discarded its final Pong on stdin EOF")
        print("PASS real SSH pipeline: stdin EOF preserves final Pong and exits0")
        with Bridge(config) as first:
            header = first.handshake()
            history = first.subscribe(header["working_dir"], instance)
            session_id = history["session_id"]
            first.send({"type": "message", "id": 103, "content": sentinel, "images": [], "no_reply": True})
            first.until(lambda event: event.get("type") == "context_message_added" and event.get("id") == 103)
            first.send({"type": "get_history", "id": 104})
            history = first.until(lambda event: event.get("type") == "history" and event.get("id") == 104)
            require(history["session_id"] == session_id and history_contains(history, sentinel),
                    "Context-only message was not persisted in its assigned session")
        assert_no_local_transcript(home, session_id, sentinel)
        with Bridge(config) as second:
            second.handshake()
            history = second.subscribe(header["working_dir"], instance, session_id)
            require(history["session_id"] == session_id and history_contains(history, sentinel),
                    "Fresh SSH attach did not restore full remote history with same session ID")
        print(f"PASS raw SSH: protocol1, ping, context-only persist, disconnect/fresh attach; session={session_id}")
        assert_no_local_transcript(home, session_id, sentinel)

        for cwd in (config["CWD"].rstrip("/") + "/missing-" + uuid.uuid4().hex, "/dev/null"):
            result = subprocess.run(local_command(config, cwd=cwd), stdin=subprocess.DEVNULL,
                                    capture_output=True, env=env, cwd=root, timeout=TIMEOUT)
            text = visible(result.stdout + result.stderr)
            require(result.returncode != 0 and "Remote:" not in text,
                    f"Invalid remote cwd silently fell back: {cwd}\n{text}")
        for tail in (("--model", "must-not-run"), ("--tools", "none"), ("--resume",)):
            result = subprocess.run(local_command(config, tail=tail), stdin=subprocess.DEVNULL,
                                    capture_output=True, env=env, cwd=root, timeout=15)
            text = visible(result.stdout + result.stderr)
            require(result.returncode != 0 and "Connecting local Jcode UI" not in text,
                    f"Unsupported local flag was not refused before SSH: {tail}\n{text}")
        print("PASS invalid missing/non-directory remote cwd and unsupported local flags rejected")

        tui_acceptance(config, env, root, session_id, sentinel)
        assert_no_local_transcript(home, session_id, sentinel)
        # A final new SSH ping/attach proves quitting the local UI did not stop
        # the remote daemon or lose the context-only session.
        with Bridge(config) as final:
            final.handshake()
            history = final.subscribe(header["working_dir"], instance, session_id)
            require(history["session_id"] == session_id and history_contains(history, sentinel),
                    "Remote daemon/session did not survive local UI quit")
        print("PASS no local transcript, persistent remote daemon survives local UI quit")
        print(json.dumps({"status": "passed", "host": config["HOST"], "session_id": session_id,
                          "sentinel": sentinel, "remote_version": header["version"],
                          "remote_working_dir": header["working_dir"], "provider_turns_requested": 0}))


class HarnessSelfTests(unittest.TestCase):
    def test_visible_strips_terminal_controls_not_payload(self):
        self.assertEqual(visible(b"\x1b]0;title\x07\x1b[31mSSH dev\x1b[0m sentinel"), "SSH dev sentinel")

    def test_remote_command_quotes_paths_and_preserves_target(self):
        config = {"REMOTE_BINARY": "/remote/a'jcode", "CWD": "/workspace/a b'c", "SERVER_SOCKET": "/socket/a b"}
        argv = shlex.split(remote_command(config).split("exec ", 1)[1])
        self.assertEqual(argv[0], config["REMOTE_BINARY"])
        self.assertEqual(argv[argv.index("--cwd") + 1], config["CWD"])
        self.assertEqual(argv[-2:], ["server", "stdio"])

    def test_history_requires_message_content(self):
        self.assertTrue(history_contains({"messages": [{"content": "marker"}]}, "marker"))
        self.assertFalse(history_contains({"session_id": "marker", "messages": []}, "marker"))

    def test_no_local_transcript_detects_leak(self):
        with tempfile.TemporaryDirectory() as root:
            home = Path(root)
            (home / "sessions").mkdir()
            assert_no_local_transcript(home, "session_test", "marker")
            (home / "sessions" / "leak.json").write_text('{"content":"marker"}')
            with self.assertRaises(AssertionError):
                assert_no_local_transcript(home, "session_test", "marker")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        unittest.main(argv=[sys.argv[0]], verbosity=2)
    else:
        require(len(sys.argv) == 1, "Usage: test_native_ssh_cli.py [--self-test]")
        config = configured()
        if config:
            run_acceptance(config)
