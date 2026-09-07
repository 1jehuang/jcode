#!/usr/bin/env python3
"""Opt-in real local PTY -> native SSH -> remote scriptable /login acceptance.

Run offline: python3 tests/test_native_ssh_login.py --self-test
Run live only after the coordinator deploys matching binaries:
  JCODE_NATIVE_SSH_LOGIN=1 \\
  JCODE_NATIVE_SSH_BINARY=/absolute/local/jcode \\
  JCODE_NATIVE_SSH_HOST=explicit-verified-ec2-alias \\
  JCODE_NATIVE_SSH_CWD=/home/ubuntu/jcode \\
  JCODE_NATIVE_SSH_LOGIN_REMOTE_EXECUTABLE=/absolute/remote/ELF/jcode \\
    python3 tests/test_native_ssh_login.py

No opt-in means no subprocess or network access. No builds, AWS API, installation,
SSH configuration edits, browser opening, OAuth completion, or model prompts.
The remote executable MUST be an actual ELF, not a wrapper that could reset HOME.
We create our own private wrapper under ~/.cache/jcode-login-acceptance/, with an
empty HOME, JCODE_HOME, runtime and explicit private server socket. The wrapper
executes the real CLI and rejects every completion except a synthetic localhost
callback whose state demonstrably differs from the remote pending state. It also
sets a closed loopback HTTP proxy as defense in depth, not as packet-capture proof.

Acceptance contract agreed with the TUI/CLI implementers:
* /login shows the shared inline Login picker with remote provider status and
  explicit local-import choices. Arrow keys/filter/Enter navigate, Esc cancels.
* A genuinely empty remote HOME offers Yes/No import onboarding on each fresh
  attach. Yes opens the import chooser only, default No opens the full catalog.
  Canceling either path creates no credentials or pending OAuth state.
* /login openai shows the exact real VM-generated URL. Its state/PKCE challenge
  match the VM pending file, whose verifier is never returned to this machine.
* /login claude begins and cancels only. Legacy Claude puts its verifier in URL
  state, so that URL necessarily reaches the private in-memory PTY buffer. It is
  never printed or persisted by the harness. The VM audits only its hash/length
  and checks its PKCE challenge locally. The OpenAI verifier guarantee does NOT
  apply to Claude. All Claude completion inputs are refused by the wrapper.
* A synthetic callback reaches the real CLI via stdin, fails at state validation
  before token exchange, and never enters session/prompt history or log files.
* /cancel removes only the attempt's flow-id pending file. A seeded legacy pending
  file remains byte-for-byte unchanged. Errors never fall back to local login.
* /quit reaps owned SSH processes and private local adapter sockets.

The isolated remote daemon and context-only session are retained as acceptance
artifacts. Exact root/socket/observed PID + start-time identities are reported for
coordinator cleanup. Failure cleanup only removes pending state in OUR fresh root.
Raw PTY output, PKCE verifiers, callback input, and credentials are never printed.
"""

import contextlib
import errno
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import pty
import re
import selectors
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
from unittest import mock
from urllib.parse import parse_qs, urlencode, urlsplit
import uuid

import test_native_ssh_cli as native

PREFIX = native.PREFIX
require = native.require
TIMEOUT = native.TIMEOUT
INVALID_CODE_PREFIX = "JCODE_SSH_LOGIN_INVALID_"
CANCEL_COMPLETE = "Pending authorization was removed on the remote host."
CREDENTIAL_NAMES = {
    "openai-auth.json", "claude-auth.json", "auth.json", ".credentials.json",
    "openai.env", "anthropic.env", "google-tokens.json", "gemini-auth.json",
}

# This is a safety fixture, not a fake auth service. Every accepted invocation
# runs the actual deployed CLI. Login stderr is inspected only for a fixed state
# mismatch marker, never persisted or returned in an assertion diagnostic.
REMOTE_WRAPPER = r'''
import base64, hashlib, json, os, pathlib, subprocess, sys
from urllib.parse import parse_qs, urlsplit
root = pathlib.Path(ROOT)
env = {
    "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root / "home"),
    "USER": "jcode-login-acceptance", "LANG": "C.UTF-8", "TERM": "xterm-256color",
    "JCODE_HOME": str(root / "jcode"), "JCODE_RUNTIME_DIR": str(root / "runtime"),
    "XDG_RUNTIME_DIR": str(root / "runtime"),
    "XDG_CONFIG_HOME": str(root / "home" / ".config"),
    "XDG_CACHE_HOME": str(root / "home" / ".cache"),
    "JCODE_NO_BROWSER": "1", "NO_BROWSER": "1", "BROWSER": "/bin/false",
    "JCODE_NO_TELEMETRY": "1", "DO_NOT_TRACK": "1", "JCODE_WAKE_MODE": "external",
    "HTTP_PROXY": "http://127.0.0.1:9", "HTTPS_PROXY": "http://127.0.0.1:9",
    "ALL_PROXY": "http://127.0.0.1:9", "NO_PROXY": "",
}
os.umask(0o077)
args = sys.argv[1:]
if "login" not in args:
    os.execve(EXECUTABLE, [EXECUTABLE, *args], env)

def option(name):
    return args[args.index(name) + 1] if name in args and args.index(name) + 1 < len(args) else None

def refuse():
    print("Acceptance safety guard refused login invocation", file=sys.stderr)
    sys.exit(93)

provider = option("--provider")
if provider not in ("openai", "claude") or not option("--flow-id"):
    refuse()
flow = option("--flow-id")
import re
if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", flow):
    refuse()
if "--complete" in args or any(arg.startswith("--auth-code") for arg in args):
    refuse()
if provider == "claude" and any(arg.startswith("--callback-url") for arg in args):
    refuse()
record = {"flow_id": flow, "provider": provider}
payload = None
if "--callback-url" in args:
    if option("--callback-url") != "-":
        refuse()
    record["operation"] = "callback_stdin"
    payload = sys.stdin.buffer.readline(16385)
    try:
        parsed = urlsplit(payload.decode().strip())
        query = parse_qs(parsed.query, strict_parsing=True)
        states = [json.loads(p.read_text())["login"]["state"]
                  for p in (root / "jcode" / "pending-login").rglob("openai.json")]
        if (len(payload) > 16384 or parsed.scheme != "http" or
            parsed.hostname not in ("localhost", "127.0.0.1") or parsed.path != "/auth/callback" or
            len(query.get("state", [])) != 1 or len(query.get("code", [])) != 1 or
            not query["code"][0].startswith("JCODE_SSH_LOGIN_INVALID_") or not states or
            query["state"][0] in states):
            refuse()
    except (ValueError, KeyError, UnicodeError):
        refuse()
    record["mismatch_verified"] = True
elif "--print-auth-url" in args:
    record["operation"] = "start"
elif "--cancel" in args:
    record["operation"] = "cancel"
else:
    refuse()
flag = root / "fail-next-login"
if record["operation"] == "start" and flag.exists():
    # Deliberately sensitive stderr proves the TUI does not echo raw subprocess
    # errors. This is not an OAuth exchange or an application event mock.
    message = flag.read_bytes()
    flag.unlink()
    with (root / "login-audit.jsonl").open("a") as out:
        out.write(json.dumps({"operation": "injected_start_failure", "flow_id": flow,
                              "provider": provider, "exit_code": 72}) + "\n")
    sys.stderr.buffer.write(message)
    sys.exit(72)
result = subprocess.run([EXECUTABLE, *args], input=payload if payload is not None else b"",
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
record["exit_code"] = result.returncode
record["state_mismatch"] = b"OAuth state mismatch" in result.stderr
if record["operation"] == "start" and result.returncode == 0:
    try:
        prompt = json.loads(result.stdout)
        url = prompt["auth_url"]
        if provider == "openai":
            record["auth_url"] = url
        else:
            # Claude's legacy URL state equals its verifier. Never persist the
            # URL or return the verifier separately as observation metadata.
            login = json.loads((root / "jcode" / "pending-login" / "flows" /
                                flow / "claude.json").read_text())["login"]
            query = parse_qs(urlsplit(url).query)
            challenge = base64.urlsafe_b64encode(hashlib.sha256(
                login["verifier"].encode()).digest()).decode().rstrip("=")
            record["auth_url_sha256"] = hashlib.sha256(url.encode()).hexdigest()
            record["auth_url_length"] = len(url)
            record["pkce_matches"] = query.get("code_challenge") == [challenge]
            record["legacy_state_is_verifier"] = query.get("state") == [login["verifier"]]
    except (ValueError, KeyError):
        pass
with (root / "login-audit.jsonl").open("a") as out:
    out.write(json.dumps(record) + "\n")
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
sys.exit(result.returncode)
'''

# Executed by the remote system Python, not by jcode. It observes only the fresh
# acceptance root. No pending verifier or file content is sent back over SSH.
REMOTE_CONTROL = r'''
import base64, hashlib, json, os, pathlib, pwd, socket, stat, struct, tempfile, time
request = REQUEST
operation = request["operation"]
if operation == "create":
    executable = pathlib.Path(request["executable"])
    assert executable.is_absolute() and executable.is_file() and os.access(executable, os.X_OK)
    with executable.open("rb") as source:
        assert source.read(4) == b"\x7fELF", "Expected actual ELF, not a remote HOME-overriding wrapper"
    parent = pathlib.Path(pwd.getpwuid(os.getuid()).pw_dir) / ".cache" / "jcode-login-acceptance"
    parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    root = pathlib.Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    for name in ("home", "jcode", "runtime"):
        (root / name).mkdir(mode=0o700)
    (root / ".acceptance-owned").write_text(request["owner"])
    wrapper = root / "remote-jcode"
    wrapper.write_text("#!/usr/bin/python3\nROOT = " + repr(str(root)) +
                       "\nEXECUTABLE = " + repr(str(executable)) + "\n" + request["wrapper"])
    wrapper.chmod(0o700)
    pending = root / "jcode" / "pending-login"
    pending.mkdir(mode=0o700)
    # A disposable stand-in for another invocation's pending state.
    legacy = {"expires_at_ms": int(time.time() * 1000) + 3600000,
              "login": {"provider": "openai", "account_label": "acceptance-legacy",
                        "verifier": "legacy-never-use-verifier", "state": "legacy-never-use-state",
                        "redirect_uri": "http://localhost:1455/auth/callback"}}
    old = pending / "openai.json"
    old.write_text(json.dumps(legacy))
    old.chmod(0o600)
    print(json.dumps({"root": str(root), "wrapper": str(wrapper),
                      "socket": str(root / "runtime" / "server.sock"),
                      "legacy_sha256": hashlib.sha256(old.read_bytes()).hexdigest()}))
else:
    root = pathlib.Path(request["root"])
    assert root.is_absolute() and root.name.startswith("run-")
    assert root.parent.name == "jcode-login-acceptance"
    assert (root / ".acceptance-owned").read_text() == request["owner"]
    pending = root / "jcode" / "pending-login"
    if operation == "cleanup_pending":
        for path in pending.rglob("*.json"):
            assert path.resolve().is_relative_to(root.resolve())
            path.unlink()
        print(json.dumps({"cleaned": True}))
    elif operation == "inject_failure":
        (root / "fail-next-login").write_text(request["error_marker"])
        print(json.dumps({"armed": True}))
    elif operation == "inspect":
        records = []
        for path in (pending / "flows").rglob("*.json"):
            if path.name not in ("openai.json", "claude.json"):
                continue
            item = json.loads(path.read_text())
            login = item["login"]
            record = {"relative_path": str(path.relative_to(root / "jcode")),
                      "mode": stat.S_IMODE(path.stat().st_mode), "provider": path.stem}
            if path.stem == "openai":
                record.update(state=login["state"], redirect_uri=login["redirect_uri"],
                              challenge=base64.urlsafe_b64encode(hashlib.sha256(
                                  login["verifier"].encode()).digest()).decode().rstrip("="))
            records.append(record)
        leaks, credentials = [], []
        needles = [value.encode() for value in request.get("needles", [])]
        for path in root.rglob("*"):
            if path.is_symlink() or not path.is_file():
                continue
            assert path.stat().st_size <= 64 * 1024 * 1024, "Oversized acceptance artifact"
            relative = str(path.relative_to(root))
            if path.name in request["credential_names"]:
                credentials.append(relative)
            if needles and any(value in path.read_bytes() for value in needles):
                leaks.append(relative)
        old = pending / "openai.json"
        legacy = hashlib.sha256(old.read_bytes()).hexdigest() if old.exists() else None
        audit = root / "login-audit.jsonl"
        calls = [json.loads(line) for line in audit.read_text().splitlines()] if audit.exists() else []
        processes = []
        server_socket = root / "runtime" / "server.sock"
        if server_socket.is_socket():
            # Spawned daemons inherit the socket through their environment, not
            # necessarily argv. Kernel peer credentials identify the listener
            # without /proc environ access or broad process-name matching.
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(5)
                connection.connect(str(server_socket))
                pid, uid, gid = struct.unpack("3i", connection.getsockopt(
                    socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
            path = pathlib.Path("/proc") / str(pid)
            args = (path / "cmdline").read_bytes().split(b"\0")
            fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
            processes.append({"pid": pid, "start_time": fields[19], "uid": uid,
                              "executable": os.fsdecode(args[0]), "server": True,
                              "identity_source": "isolated Unix socket SO_PEERCRED"})
        print(json.dumps({"pending": records, "legacy_sha256": legacy, "calls": calls,
                          "leaks": leaks, "credentials": credentials, "processes": processes}))
    else:
        raise AssertionError("Unknown acceptance control operation")
'''


def configured():
    if os.environ.get(PREFIX + "LOGIN") != "1":
        print("SKIP native SSH login acceptance: set JCODE_NATIVE_SSH_LOGIN=1 explicitly")
        return None
    executable = os.environ.get(PREFIX + "LOGIN_REMOTE_EXECUTABLE", "")
    require(executable.startswith("/") and not any(ord(c) < 32 or ord(c) == 127 for c in executable),
            "LOGIN_REMOTE_EXECUTABLE must name an absolute remote ELF")
    # Reuse host/path/executable validation without accepting a user-supplied
    # REMOTE_BINARY wrapper or preexisting server socket for this auth test.
    with mock.patch.dict(os.environ, {PREFIX + "REMOTE_BINARY": executable}):
        config = native.configured()
    require(config is not None, "Missing native SSH login opt-in configuration")
    config["SERVER_SOCKET"] = None
    config["EXECUTABLE"] = executable
    return config


def remote_control(config, operation, **values):
    request = {"operation": operation, "owner": config["OWNER"], **values}
    if config.get("ROOT"):
        request["root"] = config["ROOT"]
    script = "REQUEST = " + repr(request) + "\n" + REMOTE_CONTROL
    result = subprocess.run(["ssh", *native.SSH_FLAGS, "--", config["HOST"], "python3 -"],
                            input=script.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=TIMEOUT)
    require(result.returncode == 0, f"Remote acceptance {operation} failed (exit {result.returncode}); output withheld")
    require(len(result.stdout) <= native.MAX_FRAME, "Oversized remote acceptance metadata")
    return json.loads(result.stdout)


def inspect_remote(config, needles=()):
    result = remote_control(config, "inspect", needles=list(needles), credential_names=sorted(CREDENTIAL_NAMES))
    require(not result["leaks"], "Sensitive input leaked into isolated remote files: " + str(result["leaks"]))
    require(not result["credentials"], "Unexpected remote credentials: " + str(result["credentials"]))
    require(result["legacy_sha256"] == config["LEGACY_SHA256"], "Login changed another invocation's pending state")
    return result


def wait_remote_call(config, tui, operation, timeout=TIMEOUT, provider=None):
    """Do not mistake a repaint of an old error for a new subprocess result.

    This polls the real remote wrapper audit, not a replacement protocol event.
    Keep pumping the real PTY while waiting so a blocked render cannot stall SSH.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        tui.pump(0.2)
        snapshot = remote_control(config, "inspect", needles=[], credential_names=sorted(CREDENTIAL_NAMES))
        if any(call["operation"] == operation and (provider is None or call["provider"] == provider)
               for call in snapshot["calls"]):
            return snapshot
        require(tui.process.poll() is None, "Login PTY exited before remote subprocess result")
    raise AssertionError("Remote login subprocess did not record " + operation)


def assert_local_clean(root, needles=()):
    for path in root.rglob("*"):
        if path.is_symlink() or not path.is_file():
            continue
        relative = path.relative_to(root)
        require("pending-login" not in relative.parts and path.name not in CREDENTIAL_NAMES,
                f"Remote login created local auth material: {relative}")
        require(path.stat().st_size <= 64 * 1024 * 1024, "Oversized local acceptance artifact")
        require(not any(value.encode() in path.read_bytes() for value in needles),
                f"Sensitive callback leaked into local file: {relative}")


def invalid_callback(pending):
    state = "not-the-remote-state-" + uuid.uuid4().hex
    require(state != pending["state"], "Synthetic callback unexpectedly matches remote OAuth state")
    code = INVALID_CODE_PREFIX + uuid.uuid4().hex
    callback = "http://localhost:1455/auth/callback?" + urlencode({"code": code, "state": state})
    return callback, code


def verify_prompt(pending, call):
    require(pending["mode"] == 0o600, "Remote pending OAuth file is not private0600")
    require(pending["relative_path"] == f"pending-login/flows/{call['flow_id']}/openai.json",
            "Pending login is not scoped to the TUI flow-id")
    url = call.get("auth_url", "")
    parsed = urlsplit(url)
    query = parse_qs(parsed.query)
    require(parsed.scheme == "https" and parsed.hostname == "auth.openai.com", "Not a real OpenAI authorization URL")
    for key, expected in (("state", pending["state"]), ("code_challenge", pending["challenge"]),
                          ("redirect_uri", pending["redirect_uri"]), ("code_challenge_method", "S256")):
        require(query.get(key) == [expected], f"VM auth URL does not match remote pending {key}")
    return url


def private_url_match(text, call):
    length = call["auth_url_length"]
    require(0 < length <= 16384, "Invalid private authorization URL length")
    for match in re.finditer("https://", text):
        candidate = text[match.start():match.start() + length]
        if hashlib.sha256(candidate.encode()).hexdigest() == call["auth_url_sha256"]:
            return candidate
    return None


class LoginPTY:
    """Real CLI in a real PTY. No application-event injection or model messages."""

    def __init__(self, config, env, cwd, session_id):
        self.config = config
        self.socket_temp = tempfile.TemporaryDirectory(prefix="jlogin-", dir="/tmp")
        self.output = bytearray()
        self.ssh_children, self.sockets, self.answered = set(), set(), set()
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 60, 600, 0, 0))
        try:
            self.process = subprocess.Popen(native.local_command(config, tail=("--resume", session_id)),
                                            stdin=slave, stdout=slave, stderr=slave,
                                            env=dict(env, TMPDIR=self.socket_temp.name), cwd=cwd,
                                            preexec_fn=native.child_terminal, close_fds=True)
        finally:
            os.close(slave)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.master, selectors.EVENT_READ)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=7)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        for pid, started in self.ssh_children:
            info = native.process_info(pid)
            if info and info[1] == started:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGKILL)
        self.selector.close()
        os.close(self.master)
        self.socket_temp.cleanup()

    def pump(self, duration=0.15):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            self.ssh_children.update(native.owned_ssh(self.process.pid))
            self.sockets.update(native.owned_sockets(self.socket_temp.name))
            for _, _ in self.selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                try:
                    chunk = os.read(self.master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        return
                    raise
                if not chunk:
                    return
                self.output.extend(chunk)
                require(len(self.output) <= 32 * 1024 * 1024, "Unbounded login PTY output")
                for query, reply in [(b"\x1b[6n", b"\x1b[1;1R"), (b"\x1b[?u", b"\x1b[?0u"),
                                     (b"\x1b[c", b"\x1b[?1;2c"), (b"\x1b[>c", b"\x1b[>0;0;0c")]:
                    for match in re.finditer(re.escape(query), self.output):
                        token = (query, match.start())
                        if token not in self.answered:
                            os.write(self.master, reply)
                            self.answered.add(token)

    def wait(self, marker, since=0, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            text = native.visible(self.output[since:])
            if marker in text:
                return text
            require(self.process.poll() is None, "Login PTY exited before expected marker; raw output withheld")
        raise AssertionError("Login PTY timed out waiting for " + marker.split("https://")[0] + "; raw output withheld")

    def command(self, command):
        require(command in {"/login", "/login openai", "/login claude", "/login openai-api", "/login not-a-provider", "/cancel", "/quit"},
                "Harness permits only non-inference login/quit commands")
        self.pump(0.2)
        mark = len(self.output)
        os.write(self.master, command.encode() + b"\r")
        return mark

    def choose_startup_import(self, accept=False):
        """Respond only to the real empty-host offer, never to credential consent."""
        host = self.config["HOST"]
        self.wait(f"No logins are configured on {host}. Import a local login first?")
        self.pump(0.2)
        mark = len(self.output)
        # Enter alone exercises default No. Yes opens a chooser, not a transfer.
        os.write(self.master, b"yes\r" if accept else b"\r")
        self.wait(f"Login on {host}:", mark)
        if accept:
            self.wait("Import local OpenAI login", mark)
            self.wait("Import local Claude login", mark)
        else:
            self.wait("Anthropic/Claude", mark)
        return mark

    def dismiss_startup_import(self):
        self.choose_startup_import(False)
        mark = self.command("/cancel")
        self.wait("No authorization was started.", mark)

    def check_catalog_choices(self):
        # The full catalog exceeds the viewport. Filter rather than depending on
        # fixed row positions or expecting offscreen providers in terminal output.
        for query, labels in (
            ("openai api", ("OpenAI API", "API key")),
            ("copilot", ("GitHub Copilot", "device code")),
            ("import local", ("Import local OpenAI login", "Import local Claude login")),
        ):
            mark = len(self.output)
            os.write(self.master, b"\x1b[200~" + query.encode() + b"\x1b[201~")
            for label in labels:
                self.wait(label, mark)
            os.write(self.master, b"\x1b")  # Clear nonempty filter, keep picker open.
            self.pump(0.2)

    def private_url(self, call, since=0):
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            self.pump()
            url = private_url_match(native.visible(self.output[since:]), call)
            if url:
                return url
            require(self.process.poll() is None, "Login PTY exited before private authorization URL")
        raise AssertionError("Login PTY did not display the hash-matched private URL; output withheld")

    def callback(self, value, pending):
        parsed = urlsplit(value)
        query = parse_qs(parsed.query)
        require(parsed.hostname == "localhost" and query.get("state") != [pending["state"]]
                and query.get("code", [""])[0].startswith(INVALID_CODE_PREFIX),
                "Only intentionally invalid synthetic callback input is permitted")
        self.pump(0.2)
        mark = len(self.output)
        # Bracketed paste exercises the actual terminal's sensitive-input route.
        os.write(self.master, b"\x1b[200~" + value.encode() + b"\x1b[201~\r")
        return mark

    def quit(self):
        self.command("/quit")
        deadline = time.monotonic() + 15
        while self.process.poll() is None and time.monotonic() < deadline:
            self.pump()
        require(self.process.poll() == 0, "Login PTY did not exit0 after /quit")
        require(self.ssh_children and self.sockets, "Did not observe real SSH children and native adapter sockets")
        for pid, started in self.ssh_children:
            info = native.process_info(pid)
            require(info is None or info[1] != started, "Owned SSH process survived /quit")
        require(all(not path.exists() and not path.parent.exists() for path in self.sockets),
                "Local native adapter socket/directory survived /quit")


def run_acceptance(config):
    require(sys.platform.startswith("linux"), "Login PTY acceptance needs Linux /proc")
    config = dict(config, OWNER=uuid.uuid4().hex)
    created = remote_control(config, "create", executable=config["EXECUTABLE"], wrapper=REMOTE_WRAPPER)
    config.update(ROOT=created["root"], REMOTE_BINARY=created["wrapper"],
                  SERVER_SOCKET=created["socket"], LEGACY_SHA256=created["legacy_sha256"])
    print(json.dumps({"isolated_remote_root": config["ROOT"], "server_socket": config["SERVER_SOCKET"],
                      "owner": config["OWNER"], "cleanup": "coordinator owns isolated daemon/artifacts"}))
    try:
        with tempfile.TemporaryDirectory(prefix="jcode-ssh-login-", dir=os.environ.get("JCODE_SCRATCH_DIR")) as root:
            root = Path(root)
            for name in ("home", "jcode", "runtime"):
                (root / name).mkdir(mode=0o700)
            env = {key: value for key, value in os.environ.items()
                   if key in {"PATH", "USER", "LOGNAME", "LANG", "LC_ALL", "SSH_AUTH_SOCK", "SSH_AGENT_PID"}}
            env.update(HOME=str(root / "home"), JCODE_HOME=str(root / "jcode"),
                       JCODE_RUNTIME_DIR=str(root / "runtime"), XDG_RUNTIME_DIR=str(root / "runtime"),
                       XDG_CONFIG_HOME=str(root / "home" / ".config"),
                       XDG_CACHE_HOME=str(root / "home" / ".cache"),
                       JCODE_NO_BROWSER="1", NO_BROWSER="1", BROWSER="/bin/false",
                       JCODE_NO_TELEMETRY="1", DO_NOT_TRACK="1", JCODE_WAKE_MODE="external",
                       TERM="xterm-256color", NO_COLOR="0")
            sentinel = "JCODE_SSH_LOGIN_CONTEXT_" + uuid.uuid4().hex
            instance = "ssh-login-acceptance-" + uuid.uuid4().hex
            with native.Bridge(config) as bridge:
                header = bridge.handshake()
                history = bridge.subscribe(header["working_dir"], instance)
                session_id = history["session_id"]
                bridge.send({"type": "message", "id": 103, "content": sentinel, "images": [], "no_reply": True})
                bridge.until(lambda event: event.get("type") == "context_message_added" and event.get("id") == 103)
            needles = []
            for accept in (True, False):
                with LoginPTY(config, env, root, session_id) as tui:
                    tui.wait(f"SSH {config['HOST']}")
                    tui.wait(sentinel)
                    tui.choose_startup_import(accept)
                    if not accept:
                        tui.check_catalog_choices()
                    snapshot = inspect_remote(config)
                    require(not snapshot["pending"] and not snapshot["credentials"]
                            and not snapshot["calls"], "Startup choice started authentication or copied credentials")
                    assert_local_clean(root)
                    mark = tui.command("/cancel")
                    tui.wait("No authorization was started.", mark)
                    tui.quit()
                print("PASS empty-host startup " + ("Yes opens import chooser" if accept else "default No opens catalog")
                      + ": no credentials, OAuth, or model turns")
            with LoginPTY(config, env, root, session_id) as tui:
                tui.wait(f"SSH {config['HOST']}")
                tui.wait(sentinel)
                tui.dismiss_startup_import()
                mark = tui.command("/login")
                tui.wait(f"Login on {config['HOST']}:", mark)
                tui.check_catalog_choices()
                require(not inspect_remote(config)["pending"], "Bare /login started OAuth before provider choice")
                mark = tui.command("/cancel")
                tui.wait("No authorization was started.", mark)

                mark = tui.command("/login openai-api")
                tui.wait(f"OpenAI API setup on {config['HOST']}", mark)
                tui.wait("no local credentials were accessed", mark)
                require(not inspect_remote(config)["pending"], "Unsupported API route started OAuth")

                mark = tui.command("/login openai")
                tui.wait("https://auth.openai.com/", mark)
                snapshot = inspect_remote(config)
                require(len(snapshot["pending"]) == 1, "Expected exactly one isolated pending OpenAI flow")
                pending = snapshot["pending"][0]
                starts = [call for call in snapshot["calls"] if call["operation"] == "start"]
                require(len(starts) == 1 and starts[0]["exit_code"] == 0, "Real remote CLI did not start OAuth")
                tui.wait(verify_prompt(pending, starts[0]), mark)
                assert_local_clean(root)
                print("PASS /login choices and actual VM-generated OAuth URL match remote private flow state")

                callback, code = invalid_callback(pending)
                needles += [callback, code]
                mark = tui.callback(callback, pending)
                tui.wait("SSH login failed", mark)
                require(code not in native.visible(tui.output), "Sensitive callback was displayed instead of masked")
                snapshot = inspect_remote(config, needles)
                completions = [call for call in snapshot["calls"] if call["operation"] == "callback_stdin"]
                require(len(completions) == 1 and completions[0]["exit_code"] != 0
                        and completions[0]["mismatch_verified"] and completions[0]["state_mismatch"],
                        "Synthetic stdin callback did not fail in the real CLI before token exchange")
                assert_local_clean(root, needles)
                mark = tui.command("/cancel")
                tui.wait(CANCEL_COMPLETE, mark)
                cancelled = inspect_remote(config, needles)
                require(not cancelled["pending"], "/cancel left owned remote pending OAuth state")
                require(any(call["operation"] == "cancel" and call["exit_code"] == 0
                            and call["flow_id"] == starts[0]["flow_id"] for call in cancelled["calls"]),
                        "/cancel did not invoke the real remote CLI for the same flow-id")
                print("PASS sensitive stdin callback refused at state check, no local auth/files, /cancel clears only owned flow")
                tui.quit()

            # Login display messages are not persisted into remote history.
            # Fresh real PTYs prevent old cancellation/error redraws from being
            # mistaken for the next operation's acknowledgement. Each /quit
            # still checks owned SSH children and native socket cleanup.
            with LoginPTY(config, env, root, session_id) as tui:
                tui.wait(f"SSH {config['HOST']}")
                tui.wait(sentinel)
                tui.dismiss_startup_import()
                mark = tui.command("/login not-a-provider")
                tui.wait("SSH login supports:", mark)
                require(not inspect_remote(config, needles)["pending"], "Unsupported provider created pending state")
                # Make the next actual SSH login subprocess fail. Do not replace
                # SSH, the local CLI, or the TUI with a mocked event source.
                error_marker = "DO_NOT_ECHO_REMOTE_AUTH_ERROR_" + uuid.uuid4().hex
                remote_control(config, "inject_failure", error_marker=error_marker)
                mark = tui.command("/login openai")
                wait_remote_call(config, tui, "injected_start_failure")
                tui.wait("SSH login failed", mark)
                require(error_marker not in native.visible(tui.output), "TUI exposed sensitive remote stderr")
                needles.append(error_marker)
                require(not inspect_remote(config, needles)["pending"], "Failed remote login left pending state")
                mark = tui.command("/cancel")
                tui.wait(CANCEL_COMPLETE, mark)
                tui.quit()

            with LoginPTY(config, env, root, session_id) as tui:
                tui.wait(f"SSH {config['HOST']}")
                tui.wait(sentinel)
                tui.dismiss_startup_import()
                # Claude initiation is network-free, but its legacy URL carries
                # its PKCE verifier in state. Keep that URL private and never
                # send ANY completion input, synthetic or otherwise, for Claude.
                mark = tui.command("/login claude")
                claude = wait_remote_call(config, tui, "start", provider="claude")
                starts = [call for call in claude["calls"]
                          if call["operation"] == "start" and call["provider"] == "claude"]
                require(len(starts) == 1 and starts[0]["exit_code"] == 0,
                        "Real remote Claude CLI did not begin authorization")
                call = starts[0]
                require(call.get("pkce_matches") and call.get("legacy_state_is_verifier"),
                        "Claude URL did not match its VM-side pending PKCE state")
                require(len(claude["pending"]) == 1 and claude["pending"][0]["mode"] == 0o600
                        and claude["pending"][0]["relative_path"] ==
                        f"pending-login/flows/{call['flow_id']}/claude.json",
                        "Claude pending state was not private and scoped to the attempt")
                private_url = tui.private_url(call, mark)
                needles.append(private_url)
                assert_local_clean(root, needles)
                inspect_remote(config, needles)
                mark = tui.command("/cancel")
                tui.wait(CANCEL_COMPLETE, mark)
                cancelled = inspect_remote(config, needles)
                require(not cancelled["pending"] and any(
                    entry["operation"] == "cancel" and entry["provider"] == "claude"
                    and entry["flow_id"] == call["flow_id"] and entry["exit_code"] == 0
                    for entry in cancelled["calls"]), "Claude /cancel did not remove its exact pending flow")
                print("PASS Claude begin/cancel, VM-side PKCE match, URL kept private; legacy Claude URL state contains verifier")
                tui.quit()

            assert_local_clean(root, needles)
            native.assert_no_local_transcript(root / "jcode", session_id, sentinel)
            with native.Bridge(config) as bridge:
                bridge.handshake()
                history = bridge.subscribe(header["working_dir"], instance, session_id)
                require(native.history_contains(history, sentinel), "Remote context was lost during login")
                require(not any(native.history_contains(history, needle) for needle in needles),
                        "Sensitive callback was persisted into remote transcript")
                require(not any(message.get("role") == "assistant" for message in history.get("messages", [])),
                        "Login acceptance unexpectedly produced an assistant/model turn")
                require(all(sentinel in json.dumps(message) for message in history.get("messages", [])
                            if message.get("role") == "user"),
                        "A login command was incorrectly submitted as a remote user/model turn")
            final = inspect_remote(config, needles)
            daemons = [process for process in final["processes"] if process["server"]]
            require(daemons, "Could not identify isolated daemon through its private socket")
            print("PASS remote errors redacted, no transcript leak, /quit reaps owned SSH and adapter sockets")
            print(json.dumps({"status": "passed", "host": config["HOST"], "session_id": session_id,
                              "remote_version": header["version"], "isolated_remote_root": config["ROOT"],
                              "server_socket": config["SERVER_SOCKET"], "daemon_identities": daemons,
                              "provider_turns_requested": 0, "oauth_completed": False,
                              "claude": "begin/cancel only; legacy verifier-bearing URL stays in private PTY memory",
                              "token_exchange": "refused by real CLI state validation before exchange"}))
    finally:
        # No server-stop command, directory deletion, or process kill remotely.
        # This can only clear our marked fresh root, including the legacy fixture.
        failing = sys.exc_info()[0] is not None
        try:
            state = remote_control(config, "inspect", needles=[], credential_names=sorted(CREDENTIAL_NAMES))
            print(json.dumps({"isolated_remote_root": config["ROOT"],
                              "server_socket": config["SERVER_SOCKET"],
                              "daemon_identities": state["processes"]}))
        except Exception:
            print("WARNING: could not identify isolated daemon. Coordinator must inspect reported root/socket.", file=sys.stderr)
        try:
            remote_control(config, "cleanup_pending")
        except Exception:
            if not failing:
                raise
            print("WARNING: isolated pending cleanup failed. Coordinator must clean the reported acceptance root.", file=sys.stderr)


class HarnessSelfTests(unittest.TestCase):
    def test_cancel_waits_are_unique_and_scenarios_use_fresh_ptys(self):
        import ast
        import inspect
        tree = ast.parse(inspect.getsource(run_acceptance))
        calls = [node for node in ast.walk(tree) if isinstance(node, ast.Call)]
        ptys = [node for node in calls if isinstance(node.func, ast.Name) and node.func.id == "LoginPTY"]
        self.assertEqual(len(ptys), 4)  # Startup loop plus three OAuth/error scenarios.
        waits = [node for node in calls if isinstance(node.func, ast.Attribute) and node.func.attr == "wait"]
        self.assertEqual(sum(isinstance(node.args[0], ast.Name) and node.args[0].id == "CANCEL_COMPLETE"
                             for node in waits), 3)
        self.assertFalse(any(isinstance(node.args[0], ast.Constant) and node.args[0].value == "SSH login cancelled"
                             for node in waits))
        self.assertNotIn(CANCEL_COMPLETE, "SSH login cancelled. No authorization was started.")
        self.assertNotIn(CANCEL_COMPLETE, "SSH login cancelled locally, but remote cleanup could not be confirmed.")

    def test_startup_choice_waits_for_offer_and_never_confirms_transfer(self):
        for accept in (False, True):
            tui = object.__new__(LoginPTY)
            tui.master, tui.output = 123, bytearray()
            tui.config = {"HOST": "test-remote"}
            tui.pump, tui.wait = mock.Mock(), mock.Mock()
            with mock.patch("os.write") as write:
                tui.choose_startup_import(accept)
            write.assert_called_once_with(123, b"yes\r" if accept else b"\r")
            self.assertEqual(tui.wait.call_args_list[0].args,
                             ("No logins are configured on test-remote. Import a local login first?",))
            self.assertEqual(tui.wait.call_args_list[1].args, ("Login on test-remote:", 0))
            tui.wait.side_effect = AssertionError("No real startup offer")
            with mock.patch("os.write") as write, self.assertRaises(AssertionError):
                tui.choose_startup_import(accept)
            write.assert_not_called()

    def test_catalog_checks_filter_without_selecting_authentication(self):
        tui = object.__new__(LoginPTY)
        tui.master, tui.output = 123, bytearray()
        tui.pump, tui.wait = mock.Mock(), mock.Mock()
        with mock.patch("os.write") as write:
            tui.check_catalog_choices()
        payloads = [call.args[1] for call in write.call_args_list]
        self.assertEqual(payloads, [
            b"\x1b[200~openai api\x1b[201~", b"\x1b",
            b"\x1b[200~copilot\x1b[201~", b"\x1b",
            b"\x1b[200~import local\x1b[201~", b"\x1b",
        ])
        self.assertNotIn(b"\r", b"".join(payloads))

    def exercise_wrapper(self, root, flags, payload=b"", result=None, provider="openai"):
        argv = ["remote-jcode", "login", "--provider", provider, "--flow-id", "test-flow", *flags]
        result = result or subprocess.CompletedProcess([], 1, b"", b"OAuth state mismatch")
        stdout, stderr = io.BytesIO(), io.BytesIO()
        with mock.patch.object(sys, "argv", argv), \
             mock.patch.object(sys, "stdin", mock.Mock(buffer=io.BytesIO(payload))), \
             mock.patch.object(sys, "stdout", mock.Mock(buffer=stdout)), \
             mock.patch.object(sys, "stderr", mock.Mock(buffer=stderr)), \
             mock.patch.object(os, "umask"), \
             mock.patch.object(os, "execve") as execute, \
             mock.patch.object(subprocess, "run", return_value=result) as run:
            with self.assertRaises(SystemExit) as exit_status:
                exec(compile(REMOTE_WRAPPER, "remote-wrapper", "exec"),
                     {"ROOT": str(root), "EXECUTABLE": "/unused-real-cli"})
        execute.assert_not_called()
        return exit_status.exception.code, run, stdout.getvalue()

    def test_remote_wrapper_forwards_only_mismatched_synthetic_stdin(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pending = root / "jcode" / "pending-login" / "flows" / "test-flow"
            pending.mkdir(parents=True)
            (pending / "openai.json").write_text(json.dumps({"login": {"state": "actual-remote-state"}}))
            callback, code = invalid_callback({"state": "actual-remote-state"})
            status, run, _ = self.exercise_wrapper(root, ["--callback-url", "-"], callback.encode())
            self.assertEqual(status, 1)
            self.assertEqual(run.call_args.kwargs["input"], callback.encode())
            self.assertNotIn(code, repr(run.call_args.args))
            audit = (root / "login-audit.jsonl").read_text()
            self.assertNotIn(code, audit)
            self.assertTrue(json.loads(audit)["state_mismatch"])
            self.assertTrue(json.loads(audit)["mismatch_verified"])
            env = run.call_args.kwargs["env"]
            self.assertEqual(env["JCODE_HOME"], str(root / "jcode"))
            self.assertEqual(env["HOME"], str(root / "home"))
            self.assertEqual(env["JCODE_NO_BROWSER"], "1")

    def test_remote_wrapper_refuses_matching_state_real_code_and_inline_secrets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pending = root / "jcode" / "pending-login" / "flows" / "test-flow"
            pending.mkdir(parents=True)
            (pending / "openai.json").write_text(json.dumps({"login": {"state": "actual-remote-state"}}))
            cases = [
                (["--callback-url", "-"], b"http://localhost:1455/auth/callback?code=JCODE_SSH_LOGIN_INVALID_test&state=actual-remote-state"),
                (["--callback-url", "-"], b"http://localhost:1455/auth/callback?code=real-auth-code&state=wrong"),
                (["--callback-url", "secret-inline"], b""),
                (["--auth-code", "-"], b""),
                (["--complete"], b""),
            ]
            for flags, payload in cases:
                with self.subTest(flags=flags):
                    status, run, _ = self.exercise_wrapper(root, flags, payload)
                    self.assertEqual(status, 93)
                    run.assert_not_called()
            self.assertFalse((root / "login-audit.jsonl").exists())

    def test_injected_failure_records_completion_without_persisting_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fail-next-login").write_text("sensitive-stderr-marker")
            status, run, _ = self.exercise_wrapper(root, ["--print-auth-url"])
            self.assertEqual(status, 72)
            run.assert_not_called()
            self.assertFalse((root / "fail-next-login").exists())
            audit = (root / "login-audit.jsonl").read_text()
            self.assertNotIn("sensitive-stderr-marker", audit)
            self.assertEqual(json.loads(audit)["operation"], "injected_start_failure")

    def test_claude_begin_audits_only_url_hash_and_vm_pkce_match(self):
        import base64
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pending = root / "jcode" / "pending-login" / "flows" / "test-flow"
            pending.mkdir(parents=True)
            verifier = "synthetic-claude-verifier-never-print"
            challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
            (pending / "claude.json").write_text(json.dumps({"login": {"verifier": verifier}}))
            url = "https://claude.ai/oauth/authorize?" + urlencode({"code_challenge": challenge, "state": verifier})
            response = json.dumps({"auth_url": url}).encode()
            status, run, stdout = self.exercise_wrapper(root, ["--print-auth-url"],
                result=subprocess.CompletedProcess([], 0, response, b""), provider="claude")
            self.assertEqual(status, 0)
            self.assertEqual(stdout, response)
            audit = (root / "login-audit.jsonl").read_text()
            self.assertNotIn(verifier, audit)
            self.assertNotIn(url, audit)
            record = json.loads(audit)
            self.assertNotIn("auth_url", record)
            self.assertTrue(record["pkce_matches"] and record["legacy_state_is_verifier"])
            self.assertEqual(private_url_match("before " + url + " after", record), url)
            self.assertIsNone(private_url_match("https://claude.ai/wrong", record))
            self.assertEqual(run.call_args.kwargs["input"], b"")

    def test_claude_completion_is_unconditionally_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            for flags in (["--callback-url", "-"], ["--callback-url=anything"],
                          ["--auth-code", "-"], ["--complete"]):
                with self.subTest(flags=flags):
                    status, run, _ = self.exercise_wrapper(Path(directory), flags,
                        payload=b"must-not-reach-cli", provider="claude")
                    self.assertEqual(status, 93)
                    run.assert_not_called()

    def test_claude_cancel_runs_actual_cli_without_completion_payload(self):
        with tempfile.TemporaryDirectory() as directory:
            status, run, _ = self.exercise_wrapper(Path(directory), ["--cancel"],
                result=subprocess.CompletedProcess([], 0, b'{"status":"cancelled"}', b""), provider="claude")
            self.assertEqual(status, 0)
            self.assertEqual(run.call_args.kwargs["input"], b"")
            audit = json.loads((Path(directory) / "login-audit.jsonl").read_text())
            self.assertEqual((audit["provider"], audit["operation"]), ("claude", "cancel"))

    def test_remote_error_wait_ignores_previous_callback_failure(self):
        tui = mock.Mock()
        tui.process.poll.return_value = None
        old = {"calls": [{"operation": "callback_stdin", "exit_code": 1}]}
        new = {"calls": old["calls"] + [{"operation": "injected_start_failure", "exit_code": 72}]}
        with mock.patch.dict(os.environ, {}, clear=True), \
             mock.patch(__name__ + ".remote_control", side_effect=[old, new]) as control:
            self.assertEqual(wait_remote_call({}, tui, "injected_start_failure"), new)
        self.assertEqual(control.call_count, 2)
        self.assertEqual(tui.pump.call_count, 2)

    def test_remote_metadata_never_returns_verifier_and_detects_file_leaks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "jcode-login-acceptance" / "run-test"
            pending = root / "jcode" / "pending-login" / "flows" / "test-flow"
            pending.mkdir(parents=True)
            (root / ".acceptance-owned").write_text("owner")
            (pending / "openai.json").write_text(json.dumps({"login": {
                "state": "remote-state", "verifier": "never-export-this-verifier",
                "redirect_uri": "http://localhost:1455/auth/callback"}}))
            (pending / "claude.json").write_text(json.dumps({"login": {
                "verifier": "claude-secret-verifier", "redirect_uri": "https://console.anthropic.com/oauth/code"}}))
            (root / "history.json").write_text("sensitive-callback-marker")
            request = {"operation": "inspect", "root": str(root), "owner": "owner",
                       "needles": ["sensitive-callback-marker"], "credential_names": sorted(CREDENTIAL_NAMES)}
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                exec(compile(REMOTE_CONTROL, "remote-control", "exec"), {"REQUEST": request})
            self.assertNotIn("never-export-this-verifier", output.getvalue())
            self.assertNotIn("claude-secret-verifier", output.getvalue())
            result = json.loads(output.getvalue())
            self.assertEqual(result["leaks"], ["history.json"])
            self.assertEqual(next(item for item in result["pending"] if item["provider"] == "openai")["state"], "remote-state")
            claude = next(item for item in result["pending"] if item["provider"] == "claude")
            self.assertNotIn("state", claude)
            with self.assertRaises(AssertionError):
                exec(compile(REMOTE_CONTROL, "remote-control", "exec"),
                     {"REQUEST": dict(request, operation="cleanup_pending", owner="wrong-owner")})
            self.assertTrue((pending / "openai.json").exists())

    def test_default_skip_never_starts_process(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(subprocess, "run") as run:
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertIsNone(configured())
            run.assert_not_called()

    def test_existing_native_ssh_opt_in_does_not_enable_auth_effects(self):
        with mock.patch.dict(os.environ, {PREFIX + "HOST": "production"}, clear=True):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertIsNone(configured())

    def test_missing_explicit_remote_elf_refused_offline(self):
        with mock.patch.dict(os.environ, {PREFIX + "LOGIN": "1"}, clear=True):
            with self.assertRaisesRegex(AssertionError, "absolute remote ELF"):
                configured()

    def test_invalid_callback_has_local_shape_but_never_matching_state(self):
        callback, code = invalid_callback({"state": "remote-state"})
        parsed = urlsplit(callback)
        query = parse_qs(parsed.query)
        self.assertEqual((parsed.scheme, parsed.hostname, parsed.path), ("http", "localhost", "/auth/callback"))
        self.assertTrue(code.startswith(INVALID_CODE_PREFIX))
        self.assertNotEqual(query["state"], ["remote-state"])

    def test_url_must_match_remote_pkce_state_and_flow_path(self):
        pending = {"mode": 0o600, "relative_path": "pending-login/flows/test-flow/openai.json",
                   "state": "remote-state", "challenge": "remote-challenge",
                   "redirect_uri": "http://localhost:1455/auth/callback"}
        call = {"flow_id": "test-flow", "auth_url": "https://auth.openai.com/oauth/authorize?" + urlencode({
            "state": pending["state"], "code_challenge": pending["challenge"],
            "redirect_uri": pending["redirect_uri"], "code_challenge_method": "S256"})}
        self.assertEqual(verify_prompt(pending, call), call["auth_url"])
        with self.assertRaisesRegex(AssertionError, "pending state"):
            verify_prompt(dict(pending, state="local-state"), call)
        with self.assertRaisesRegex(AssertionError, "private0600"):
            verify_prompt(dict(pending, mode=0o644), call)
        with self.assertRaisesRegex(AssertionError, "flow-id"):
            verify_prompt(dict(pending, relative_path="pending-login/openai.json"), call)

    def test_local_auth_and_prompt_history_leaks_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            assert_local_clean(root, ["sensitive-test"])
            path = root / "prompt-history.json"
            path.write_text("sensitive-test")
            with self.assertRaisesRegex(AssertionError, "Sensitive callback"):
                assert_local_clean(root, ["sensitive-test"])
            path.unlink()
            (root / "openai-auth.json").write_text("{}")
            with self.assertRaisesRegex(AssertionError, "local auth material"):
                assert_local_clean(root)

    def test_pty_rejects_arbitrary_model_prompts_before_any_write(self):
        with self.assertRaisesRegex(AssertionError, "non-inference"):
            LoginPTY.command(None, "please call a model")

    def test_remote_scripts_compile_without_running(self):
        compile("ROOT = '/unused'\nEXECUTABLE = '/unused'\n" + REMOTE_WRAPPER, "remote-wrapper", "exec")
        compile("REQUEST = {}\n" + REMOTE_CONTROL, "remote-control", "exec")

    def test_remote_control_uses_hardened_ssh_and_no_callback_in_argv(self):
        result = subprocess.CompletedProcess([], 0, b'{"cleaned":true}', b"")
        config = {"HOST": "test-host", "OWNER": "test-owner", "ROOT": "/isolated/root"}
        with mock.patch.object(subprocess, "run", return_value=result) as run:
            self.assertEqual(remote_control(config, "cleanup_pending"), {"cleaned": True})
        args, kwargs = run.call_args
        self.assertEqual(args[0], ["ssh", *native.SSH_FLAGS, "--", "test-host", "python3 -"])
        self.assertIn(b"test-owner", kwargs["input"])


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        unittest.main(argv=[sys.argv[0]], verbosity=2)
    else:
        require(len(sys.argv) == 1, "Usage: test_native_ssh_login.py [--self-test]")
        config = configured()
        if config:
            run_acceptance(config)
