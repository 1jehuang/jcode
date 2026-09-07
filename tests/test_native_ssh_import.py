#!/usr/bin/env python3
"""Opt-in real native TUI -> SSH credential import, synthetic managed stores ONLY.

Offline: python3 tests/test_native_ssh_import.py --self-test
Live, only after the coordinator builds/deploys matching binaries:
  JCODE_NATIVE_SSH_IMPORT=1 \
  JCODE_NATIVE_SSH_BINARY=/absolute/local/ELF/jcode \
  JCODE_NATIVE_SSH_IMPORT_REMOTE_EXECUTABLE=/absolute/remote/ELF/jcode \
  JCODE_NATIVE_SSH_HOST=explicit-verified-alias \
  JCODE_NATIVE_SSH_CWD=/absolute/remote/workspace \
    python3 tests/test_native_ssh_import.py

No opt-in means no subprocess/network. No Cargo, AWS, installation, personal
credential copying, OAuth, model turns, or provider validation. Local/remote HOME,
JCODE_HOME, runtime and config are fresh. Expiry is year 2100 and closed loopback
proxies block provider HTTP access (defense in depth, not packet-capture proof).
System SSH config/keys/agent remain available for the explicitly selected host.

Each provider gets its own isolated remote root with the OTHER provider seeded
independently on the VM. Bare /login and /login --import-local use the real shared
inline picker, exercising arrow and filter selection before canceling consent.
Cancellation, including highlighting Yes then selecting No with arrows, must
invoke no import and leave all remote
credentials unchanged. Confirmation must invoke the actual ELF's `auth import
--provider <id> --stdin --json`, write only the selected store with mode0600, and
leave both local stores and the remote other-provider store byte-identical.
Typed yes performs the selected synthetic import. Legacy confirm is retained
for the repeated attempt, which must refuse without overwrite. Real `auth status --json`
must see the selected configuration. A safety wrapper validates synthetic stdin,
then runs the real CLI, never a replacement auth service. Only hashes/booleans
are audited. Full tokens must not appear in argv, PTY, logs, history, or output.

PTY /quit checks reap owned SSH children and remove private adapter sockets.
Private persistent remote daemons/artifacts are retained for coordinator cleanup,
with root, owner, socket and kernel-observed PID/start-time identities reported.
No broad process killing or personal remote state cleanup is performed.
"""

import contextlib
import hashlib
import inspect
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import uuid

import test_native_ssh_cli as native
import test_native_ssh_login as login

PREFIX = native.PREFIX
require = native.require
MAX_PAYLOAD = 65536
TOKEN_PREFIX = "JCODE_SYNTHETIC_IMPORT_"
TOKEN_RE = re.compile(rb"JCODE_SYNTHETIC_IMPORT_[a-z]+_[0-9a-f]{32}")
FILES = {"openai": "openai-auth.json", "claude": "auth.json"}
EXPIRES = 4102444800000
CONFIRM = "Import your local {provider} login to {host}?"
SUCCESS = "SSH login: {provider} imported on the remote host."
# Kept in sync with auth_remote's static, secret-free UX contract.
CANCELLED = "SSH credential import cancelled"
REFUSED = "SSH credential import failed"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def fixture(provider):
    """Only generated, deliberately invalid credentials. Never load user files."""
    access = TOKEN_PREFIX + "access_" + uuid.uuid4().hex
    refresh = TOKEN_PREFIX + "refresh_" + uuid.uuid4().hex
    if provider == "openai":
        credential = {"access_token": access, "refresh_token": refresh,
                      "id_token": None, "account_id": None, "expires_at": EXPIRES}
        store = {"openai_accounts": [{"label": "openai-otter", **credential}],
                 "active_openai_account": "openai-otter"}
    else:
        require(provider == "claude", "Unsupported synthetic provider")
        credential = {"access": access, "refresh": refresh, "expires": EXPIRES,
                      "scopes": [], "subscription_type": None}
        store = {"anthropic_accounts": [{"label": "claude-otter", **credential}],
                 "active_anthropic_account": "claude-otter"}
    return store, {"version": 1, "provider": provider, "credential": credential}


def token_hashes(data):
    return sorted({digest(token) for token in TOKEN_RE.findall(data)})


def isolated_env(root):
    root = Path(root)
    return {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root / "home"),
            "USER": "jcode-import-acceptance", "LANG": "C.UTF-8", "TERM": "xterm-256color",
            "JCODE_HOME": str(root / "jcode"), "JCODE_RUNTIME_DIR": str(root / "runtime"),
            "XDG_RUNTIME_DIR": str(root / "runtime"),
            "XDG_CONFIG_HOME": str(root / "home" / ".config"),
            "XDG_CACHE_HOME": str(root / "home" / ".cache"),
            "JCODE_NO_BROWSER": "1", "NO_BROWSER": "1", "BROWSER": "/bin/false",
            "JCODE_NO_TELEMETRY": "1", "DO_NOT_TRACK": "1", "JCODE_WAKE_MODE": "external",
            "HTTP_PROXY": "http://127.0.0.1:9", "HTTPS_PROXY": "http://127.0.0.1:9",
            "ALL_PROXY": "http://127.0.0.1:9", "NO_PROXY": "",
            "http_proxy": "http://127.0.0.1:9", "https_proxy": "http://127.0.0.1:9",
            "all_proxy": "http://127.0.0.1:9", "no_proxy": ""}


def check_payload(payload, provider, expected):
    """Gate before invoking a real CLI. Errors never contain payload contents."""
    require(0 < len(payload) <= MAX_PAYLOAD, "Invalid synthetic payload size")
    item = json.loads(payload)
    require(set(item) == {"version", "provider", "credential"}
            and item["version"] == 1 and item["provider"] == provider,
            "Unexpected transfer envelope")
    credential = item["credential"]
    access, refresh, expiry = (("access_token", "refresh_token", "expires_at")
                               if provider == "openai" else ("access", "refresh", "expires"))
    allowed = ({access, refresh, expiry, "id_token", "account_id"} if provider == "openai"
               else {access, refresh, expiry, "scopes", "subscription_type"})
    require(set(credential).issubset(allowed) and credential.get(expiry) == EXPIRES,
            "Unexpected synthetic credential fields")
    for key in (access, refresh):
        require(isinstance(credential.get(key), str)
                and TOKEN_RE.fullmatch(credential[key].encode()) is not None,
                "Refusing non-synthetic credentials")
    require(all(credential.get(key) is None for key in ("id_token", "account_id", "subscription_type"))
            and credential.get("scopes", []) == [], "Unexpected credential identity")
    require(token_hashes(payload) == expected, "Payload is not selected local synthetic provider")


def scan_files(root, allowed):
    """Inspect all private artifacts, exempting only exact expected auth stores."""
    stores = {}
    for path in Path(root).rglob("*"):
        require(not path.is_symlink(), "Unexpected symlink in isolated acceptance state")
        if not path.is_file():
            continue
        require(path.stat().st_size <= 64 * 1024 * 1024, "Oversized acceptance artifact")
        relative = str(path.relative_to(root))
        data = path.read_bytes()
        hashes = token_hashes(data)
        if relative in allowed:
            stores[relative] = {"sha256": digest(data), "mode": stat.S_IMODE(path.stat().st_mode),
                                "tokens": hashes}
        else:
            require(not hashes, "Synthetic token leaked outside expected credential stores")
    return stores


# Ship only source code and expected token HASHES over setup/control stdin.
# Local source tokens do not cross SSH until the actual TUI confirmation.
def remote_common():
    return ("import hashlib,json,os,pathlib,re,stat,subprocess,sys,uuid\n"
            "from pathlib import Path\n" +
            f"MAX_PAYLOAD={MAX_PAYLOAD!r}\nTOKEN_PREFIX={TOKEN_PREFIX!r}\n"
            f"TOKEN_RE=re.compile({TOKEN_RE.pattern!r})\nFILES={FILES!r}\nEXPIRES={EXPIRES!r}\n" +
            "\n".join(inspect.getsource(fn) for fn in
                      (require, digest, fixture, token_hashes, isolated_env, check_payload, scan_files)))


REMOTE_WRAPPER = r'''
root = Path(ROOT)
os.umask(0o077)
args = sys.argv[1:]
env = isolated_env(root)
def audit(record, name="import-audit.jsonl"):
    with (root / name).open("a") as out:
        out.write(json.dumps(record) + "\n")
def refuse():
    audit({"operation": "guard_refusal"})
    print("Acceptance safety guard refused invocation", file=sys.stderr)
    sys.exit(93)
if any(TOKEN_RE.search(arg.encode()) for arg in args):
    refuse()
# Only native transport and the exact import/status forms are allowed.
index = 0
while index < len(args) and args[index].startswith("--"):
    if args[index] in ("--no-update", "--no-selfdev"):
        index += 1
    elif args[index] in ("--cwd", "--socket") and index + 1 < len(args):
        index += 2
    else:
        refuse()
tail = args[index:]
if tail == ["server", "stdio"]:
    os.execve(EXECUTABLE, [EXECUTABLE, *args], env)
if tail == ["auth", "status", "--json"]:
    os.execve(EXECUTABLE, [EXECUTABLE, *args], env)
if (len(tail) != 6 or tail[:3] != ["auth", "import", "--provider"]
        or tail[3] not in FILES or tail[4:] != ["--stdin", "--json"]):
    refuse()
provider = tail[3]
# Record BEFORE reading stdin. A canceled/blocked child cannot hide an early
# transfer merely by dying before the real CLI returns.
audit({"provider": provider}, "import-starts.jsonl")
payload = sys.stdin.buffer.read(MAX_PAYLOAD + 1)
try:
    expected = json.loads((root / "expected.json").read_text())
    check_payload(payload, provider, expected[provider])
except Exception:
    refuse()
result = subprocess.run([EXECUTABLE, *args], input=payload, env=env,
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
leak = bool(TOKEN_RE.search(result.stdout + result.stderr))
try:
    reply = json.loads(result.stdout)
    reply_status = reply.get("status")
    reply_provider = reply.get("provider")
except (ValueError, AttributeError):
    reply_status = reply_provider = None
audit({"operation": "import", "provider": provider, "stdin_bytes": len(payload),
       "selected_tokens_only": True, "exit_code": result.returncode, "output_leak": leak,
       "reply_status": reply_status if reply_status in ("imported", "error") else None,
       "reply_provider": reply_provider if reply_provider in FILES else None})
if leak:
    print("Acceptance detected secret output; contents withheld", file=sys.stderr)
    sys.exit(94)
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
sys.exit(result.returncode)
'''

REMOTE_CONTROL = r'''
import pwd, socket, struct, tempfile
request = REQUEST
os.umask(0o077)
if request["operation"] == "create":
    executable = Path(request["executable"])
    assert executable.is_absolute() and executable.is_file() and os.access(executable, os.X_OK)
    with executable.open("rb") as source:
        assert source.read(4) == b"\x7fELF", "Remote executable must be actual ELF"
    parent = Path(pwd.getpwuid(os.getuid()).pw_dir) / ".cache" / "jcode-import-acceptance"
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    for name in ("home", "jcode", "runtime"):
        (root / name).mkdir(mode=0o700)
    (root / ".acceptance-owned").write_text(request["owner"])
    (root / "expected.json").write_text(json.dumps(request["expected"]))
    other = "claude" if request["provider"] == "openai" else "openai"
    store, _ = fixture(other)  # Independent VM fixture, NOT a copy from local HOME.
    seeded = root / "jcode" / FILES[other]
    seeded.write_text(json.dumps(store))
    seeded.chmod(0o600)
    wrapper = root / "remote-jcode"
    wrapper.write_text("#!/usr/bin/python3\nROOT=" + repr(str(root)) +
                      "\nEXECUTABLE=" + repr(str(executable)) + "\n" + request["wrapper"])
    wrapper.chmod(0o700)
    print(json.dumps({"root": str(root), "wrapper": str(wrapper),
                      "socket": str(root / "runtime" / "server.sock")}))
else:
    root = Path(request["root"])
    assert root.is_absolute() and root.name.startswith("run-")
    assert root.parent.name == "jcode-import-acceptance"
    assert (root / ".acceptance-owned").read_text() == request["owner"]
    assert request["operation"] == "inspect"
    stores = scan_files(root, {"jcode/" + name for name in FILES.values()})
    audit = root / "import-audit.jsonl"
    calls = [json.loads(line) for line in audit.read_text().splitlines()] if audit.exists() else []
    starts_path = root / "import-starts.jsonl"
    starts = [json.loads(line) for line in starts_path.read_text().splitlines()] if starts_path.exists() else []
    result = subprocess.run([str(root / "remote-jcode"), "--no-update", "--no-selfdev",
                             "auth", "status", "--json"], stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
    assert result.returncode == 0, "Real remote auth status failed"
    assert not TOKEN_RE.search(result.stdout + result.stderr), "Secret in auth status output"
    status = json.loads(result.stdout)
    states = {item["id"]: item["status"] for item in status["providers"] if item["id"] in FILES}
    assert all(value in ("available", "expired", "not_configured") for value in states.values())
    processes = []
    server_socket = root / "runtime" / "server.sock"
    if server_socket.is_socket():
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(5)
            connection.connect(str(server_socket))
            pid, uid, gid = struct.unpack("3i", connection.getsockopt(
                socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
        process = Path("/proc") / str(pid)
        args = (process / "cmdline").read_bytes()
        assert not TOKEN_RE.search(args), "Secret in remote daemon argv"
        fields = (process / "stat").read_text().rsplit(")", 1)[1].split()
        processes.append({"pid": pid, "start_time": fields[19], "uid": uid,
                          "identity_source": "isolated Unix socket SO_PEERCRED"})
    # Scan again after status, which could otherwise conceal a status log leak.
    assert scan_files(root, {"jcode/" + name for name in FILES.values()}) == stores
    print(json.dumps({"stores": stores, "calls": calls, "starts": starts, "states": states, "processes": processes}))
'''


def configured():
    if os.environ.get(PREFIX + "IMPORT") != "1":
        print("SKIP native SSH import acceptance: set JCODE_NATIVE_SSH_IMPORT=1 explicitly")
        return None
    executable = os.environ.get(PREFIX + "IMPORT_REMOTE_EXECUTABLE", "")
    require(executable.startswith("/") and not any(ord(c) < 32 or ord(c) == 127 for c in executable),
            "IMPORT_REMOTE_EXECUTABLE must name an absolute remote ELF")
    with mock.patch.dict(os.environ, {PREFIX + "REMOTE_BINARY": executable}):
        config = native.configured()
    require(config is not None and Path(config["CWD"]).is_absolute(), "Explicit absolute remote CWD required")
    require(Path(os.environ[PREFIX + "BINARY"]).is_absolute(), "Local ELF path must be absolute")
    with Path(config["BINARY"]).open("rb") as source:
        require(source.read(4) == b"\x7fELF", "Local executable must be actual ELF, not wrapper")
    config.update(EXECUTABLE=executable, SERVER_SOCKET=None)
    return config


def remote_control(config, operation, **values):
    request = {"operation": operation, "owner": config["OWNER"], **values}
    if config.get("ROOT"):
        request["root"] = config["ROOT"]
    script = remote_common() + "\nREQUEST=" + repr(request) + "\n" + REMOTE_CONTROL
    result = subprocess.run(["ssh", *native.SSH_FLAGS, "--", config["HOST"], "python3 -"],
                            input=script.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=native.TIMEOUT)
    require(result.returncode == 0, "Remote import acceptance control failed; output withheld")
    require(len(result.stdout) <= native.MAX_FRAME and not TOKEN_RE.search(result.stdout + result.stderr),
            "Unsafe remote acceptance metadata; output withheld")
    return json.loads(result.stdout)


class ImportPTY(login.LoginPTY):
    def pump(self, duration=0.15):
        super().pump(duration)
        require(not TOKEN_RE.search(self.output) and not TOKEN_RE.search(native.visible(self.output).encode()),
                "Synthetic token appeared in PTY output; output withheld")
        for pid in {self.process.pid} | {pid for pid, _ in self.ssh_children}:
            info = native.process_info(pid)
            require(info is None or not any(TOKEN_RE.search(arg) for arg in info[2]),
                    "Synthetic token appeared in observed local/SSH argv")

    def command(self, command):
        require(command in {"/login", "/login --import-local",
                            "/login --import-local openai", "/login --import-local claude",
                            "/cancel", "/quit", "confirm", "yes"},
                "Only import confirmation/cancel/quit input permitted")
        self.pump(0.2)
        mark = len(self.output)
        os.write(self.master, command.encode() + b"\r")
        return mark

    def choose_import(self, provider, imports_only=False):
        """Actual terminal navigation, never injected picker/application events."""
        require(provider in FILES, "Only synthetic import provider selection permitted")
        mark = self.command("/login --import-local" if imports_only else "/login")
        self.wait(f"Login on {self.config['HOST']}:", mark)
        if not imports_only:
            # Catalog rows precede the explicit imports and may fill the viewport.
            os.write(self.master, b"\x1b[200~import local\x1b[201~")
            self.pump(0.2)
        for label in ("Import local OpenAI login", "Import local Claude login"):
            self.wait(label, mark)
        if imports_only:
            # Bracketed paste must filter the import-only picker, not bypass it
            # through the old private-input provider route into remote OAuth.
            os.write(self.master, b"\x1b[200~" + provider.encode() + b"\x1b[201~")
            self.pump(0.2)
        elif provider == "claude":
            # Filtered explicit-import rows retain OpenAI then Claude ordering.
            os.write(self.master, b"\x1b[B")
            self.pump(0.2)
        mark = len(self.output)
        os.write(self.master, b"\r")
        return mark

    def decline_with_arrows(self):
        """Prove highlighting Yes alone does nothing, then choose No with Enter."""
        self.pump(0.2)
        mark = len(self.output)
        os.write(self.master, b"\x1b[A")  # Default No -> Yes, without confirming.
        self.pump(0.2)
        os.write(self.master, b"\x1b[B\r")  # Back to No, then cancel.
        return mark

    def quit(self):
        super().quit()
        self.pump(0.2)  # Include buffered terminal bytes written just before exit.


def assert_snapshot(snapshot, provider, baseline, expected, imported, calls):
    require(len(snapshot["calls"]) == calls, "Unexpected import invocation count")
    require(len(snapshot["starts"]) == calls and all(item["provider"] == provider for item in snapshot["starts"]),
            "Unexpected started, incomplete, or wrong-provider import")
    require(all(call["operation"] == "import" and call["provider"] == provider
                and call["selected_tokens_only"] and not call["output_leak"]
                and call["reply_provider"] == provider for call in snapshot["calls"]),
            "Unsafe or wrong-provider remote invocation")
    other = "claude" if provider == "openai" else "openai"
    other_path = "jcode/" + FILES[other]
    require(snapshot["stores"].get(other_path) == baseline["stores"][other_path],
            "Other-provider remote source changed")
    selected = "jcode/" + FILES[provider]
    if imported:
        store = snapshot["stores"].get(selected, {})
        require(store.get("mode") == 0o600 and store.get("tokens") == expected[provider],
                "Selected import missing, changed, or not private0600")
        require(snapshot["states"].get(provider) == "available", "Real auth status did not see imported configuration")
    else:
        require(selected not in snapshot["stores"] and snapshot["states"].get(provider) == "not_configured",
                "Cancellation transferred credentials or changed remote configuration")


def run_provider(config, provider, root, env, local_before, expected):
    config = dict(config, OWNER=uuid.uuid4().hex)
    created = remote_control(config, "create", provider=provider, expected=expected,
                             executable=config["EXECUTABLE"], wrapper=remote_common() + REMOTE_WRAPPER)
    config.update(ROOT=created["root"], REMOTE_BINARY=created["wrapper"], SERVER_SOCKET=created["socket"])
    print(json.dumps({"isolated_remote_root": config["ROOT"], "server_socket": config["SERVER_SOCKET"],
                      "owner": config["OWNER"], "cleanup": "coordinator owns private daemon/artifacts"}))
    try:
        baseline = remote_control(config, "inspect")
        assert_snapshot(baseline, provider, baseline, expected, False, 0)
        sentinel = "JCODE_IMPORT_CONTEXT_" + uuid.uuid4().hex
        with native.Bridge(config) as bridge:
            header = bridge.handshake()
            history = bridge.subscribe(header["working_dir"], "import-" + uuid.uuid4().hex)
            session_id = history["session_id"]
            bridge.send({"type": "message", "id": 103, "content": sentinel, "images": [], "no_reply": True})
            bridge.until(lambda event: event.get("type") == "context_message_added" and event.get("id") == 103)
        after_import = None
        for action in ("picker_cancel", "filtered_cancel", "arrow_cancel", "cancel", "import", "repeat"):
            print(f"CHECK {provider} {action}", flush=True)
            # Fresh TUI prevents an old success/error repaint satisfying this step.
            with ImportPTY(config, env, root, session_id) as tui:
                tui.wait(f"SSH {config['HOST']}")
                tui.wait(sentinel)
                cancelling = action.endswith("cancel")
                if action in ("picker_cancel", "filtered_cancel"):
                    mark = tui.choose_import(provider, imports_only=action == "filtered_cancel")
                else:
                    mark = tui.command("/login --import-local " + provider)
                tui.wait(CONFIRM.format(provider=provider, host=config["HOST"]), mark)
                pending = remote_control(config, "inspect")
                before_count = 1 if action == "repeat" else 0
                assert_snapshot(pending, provider, baseline, expected, action == "repeat", before_count)
                require(scan_files(root, local_before) == local_before, "Picker/consent changed local source stores")
                require("-otter" not in native.visible(tui.output), "Picker exposed private account labels")
                if action == "arrow_cancel":
                    mark = tui.decline_with_arrows()
                else:
                    mark = tui.command("/cancel" if cancelling else
                                       ("yes" if action == "import" else "confirm"))
                marker = CANCELLED if cancelling else (SUCCESS.format(provider=provider)
                                                              if action == "import" else REFUSED)
                tui.wait(marker, mark)
                tui.quit()  # Checks real owned SSH children and native socket removal.
            snapshot = remote_control(config, "inspect")
            assert_snapshot(snapshot, provider, baseline, expected, not cancelling,
                            0 if cancelling else (1 if action == "import" else 2))
            require(scan_files(root, local_before) == local_before, "Local source stores changed")
            if action == "import":
                call = snapshot["calls"][-1]
                require(call["exit_code"] == 0 and call["reply_status"] == "imported", "Real CLI import did not succeed")
                after_import = snapshot["stores"]
            if action == "repeat":
                call = snapshot["calls"][-1]
                require(call["exit_code"] != 0 and call["reply_status"] == "error", "Repeated real CLI import did not refuse")
                require(snapshot["stores"] == after_import, "Refused import overwrote remote credentials")
            print(f"PASS {provider} {action}: selected-only stdin, source isolation, private output, child/socket cleanup")
        with native.Bridge(config) as bridge:
            bridge.handshake()
            history = bridge.subscribe(config["CWD"], "import-check-" + uuid.uuid4().hex, session_id)
            require(not TOKEN_RE.search(json.dumps(history).encode()), "Synthetic token in actual remote history")
        require(scan_files(root, local_before) == local_before, "Local source changed after reattach")
    finally:
        # Never claim remote cleanup: persistent daemons are the coordinator's.
        try:
            snapshot = remote_control(config, "inspect")
            print(json.dumps({"isolated_remote_root": config["ROOT"], "owner": config["OWNER"],
                              "server_socket": config["SERVER_SOCKET"], "processes": snapshot["processes"]}))
        except Exception:
            print("Remote identity inspection failed; use previously reported exact private root/socket for cleanup")


def run_acceptance(config):
    require(sys.platform.startswith("linux"), "Import acceptance requires Linux /proc")
    with tempfile.TemporaryDirectory(prefix="jcode-ssh-import-", dir=os.environ.get("JCODE_SCRATCH_DIR")) as directory:
        root = Path(directory)
        for name in ("home", "jcode", "runtime"):
            (root / name).mkdir(mode=0o700)
        expected = {}
        for provider, name in FILES.items():
            store, _ = fixture(provider)
            data = json.dumps(store).encode()
            path = root / "jcode" / name
            path.write_bytes(data)
            path.chmod(0o600)
            expected[provider] = token_hashes(data)
        local_before = scan_files(root, {"jcode/" + name for name in FILES.values()})
        env = isolated_env(root)
        for key in ("PATH", "LOGNAME", "SSH_AUTH_SOCK", "SSH_AGENT_PID"):
            if key in os.environ:
                env[key] = os.environ[key]
        for provider in FILES:
            run_provider(config, provider, root, env, local_before, expected)


class HarnessSelfTests(unittest.TestCase):
    def test_unset_opt_in_never_spawns(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch("subprocess.run") as run, \
                mock.patch("subprocess.Popen") as popen, contextlib.redirect_stdout(io.StringIO()):
            self.assertIsNone(configured())
            run.assert_not_called()
            popen.assert_not_called()

    def test_explicit_opt_in_and_elf_required(self):
        with tempfile.TemporaryDirectory() as directory:
            fake = Path(directory) / "wrapper"
            fake.write_text("#!/bin/sh\nexit 0\n")
            fake.chmod(0o700)
            env = {PREFIX + "IMPORT": "1", PREFIX + "BINARY": str(fake), PREFIX + "HOST": "safe-alias",
                   PREFIX + "CWD": "/workspace", PREFIX + "IMPORT_REMOTE_EXECUTABLE": "/opt/jcode"}
            with mock.patch.dict(os.environ, env, clear=True), self.assertRaisesRegex(AssertionError, "actual ELF"):
                configured()

    def test_payload_is_selected_synthetic_and_bounded(self):
        for provider in FILES:
            _, envelope = fixture(provider)
            payload = json.dumps(envelope).encode()
            expected = token_hashes(payload)
            check_payload(payload, provider, expected)
            with self.assertRaises(AssertionError):
                check_payload(payload, provider, ["wrong-provider-hash"])
            with self.assertRaises(AssertionError):
                check_payload(b" " * (MAX_PAYLOAD + 1), provider, expected)
            key = "access_token" if provider == "openai" else "access"
            envelope["credential"][key] = "not-a-synthetic-credential"
            with self.assertRaises(AssertionError):
                check_payload(json.dumps(envelope).encode(), provider, expected)

    def test_scan_all_artifacts_and_exact_store_allowlist(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "jcode").mkdir()
            store, _ = fixture("openai")
            data = json.dumps(store).encode()
            path = root / "jcode/openai-auth.json"
            path.write_bytes(data)
            path.chmod(0o600)
            allowed = {"jcode/openai-auth.json"}
            observed = scan_files(root, allowed)
            self.assertEqual(observed["jcode/openai-auth.json"]["mode"], 0o600)
            (root / "transcript.json").write_bytes(data)
            with self.assertRaisesRegex(AssertionError, "leaked"):
                scan_files(root, allowed)
            (root / "transcript.json").unlink()
            (root / "lookalike-openai-auth.json").write_bytes(data)
            with self.assertRaises(AssertionError):
                scan_files(root, allowed)

    def test_isolation_does_not_inherit_credentials_or_proxy_bypass(self):
        with mock.patch.dict(os.environ, {"OPENAI_API_KEY": "must-not-copy", "NO_PROXY": "*", "HOME": "/personal"}):
            env = isolated_env("/private")
        self.assertNotIn("OPENAI_API_KEY", env)
        self.assertEqual(env["HOME"], "/private/home")
        self.assertEqual(env["NO_PROXY"], "")
        self.assertEqual(env["HTTPS_PROXY"], "http://127.0.0.1:9")

    def test_remote_sources_compile_without_network(self):
        for source in (REMOTE_WRAPPER, REMOTE_CONTROL):
            compile(remote_common() + source, "<remote-acceptance>", "exec")

    def exercise_wrapper(self, argv, payload, expected, result):
        # Only the harness boundary is mocked. No SSH or jcode process runs in
        # selftests, and no claims about Rust behavior are inferred from these.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "expected.json").write_text(json.dumps(expected))
            streams = [io.TextIOWrapper(io.BytesIO(value), encoding="utf-8", write_through=True)
                       for value in (payload, b"", b"")]
            try:
                with mock.patch.object(sys, "argv", ["wrapper", *argv]), \
                        mock.patch.object(sys, "stdin", streams[0]), \
                        mock.patch.object(sys, "stdout", streams[1]), \
                        mock.patch.object(sys, "stderr", streams[2]), \
                        mock.patch("os.umask"), mock.patch("os.execve") as execute, \
                        mock.patch("subprocess.run", return_value=result) as run:
                    with self.assertRaises(SystemExit) as stopped:
                        exec(remote_common() + REMOTE_WRAPPER,
                             {"ROOT": str(root), "EXECUTABLE": "/not-executed/jcode"})
                    execute.assert_not_called()
                output = streams[1].buffer.getvalue() + streams[2].buffer.getvalue()
                self.assertFalse(TOKEN_RE.search(output))
                records = [json.loads(line) for line in (root / "import-audit.jsonl").read_text().splitlines()]
                self.assertFalse(TOKEN_RE.search(json.dumps(records).encode()))
                if run.called:
                    self.assertFalse(TOKEN_RE.search(json.dumps(run.call_args.args).encode()))
                    self.assertTrue(run.call_args.kwargs["input"] == payload)
                return stopped.exception.code, records, run.call_count
            finally:
                for stream in streams:
                    stream.close()

    def test_wrapper_forwards_only_selected_stdin_and_preserves_refusal(self):
        for provider in FILES:
            _, envelope = fixture(provider)
            payload = json.dumps(envelope).encode()
            expected = {provider: token_hashes(payload)}
            argv = ["--no-update", "--no-selfdev", "auth", "import", "--provider", provider, "--stdin", "--json"]
            for code, status in ((0, "imported"), (1, "error")):
                result = subprocess.CompletedProcess([], code, json.dumps({"status": status, "provider": provider}).encode(), b"")
                exit_code, records, calls = self.exercise_wrapper(argv, payload, expected, result)
                self.assertEqual((exit_code, calls), (code, 1))
                self.assertEqual(records[0]["reply_status"], status)
                self.assertTrue(records[0]["selected_tokens_only"])

    def test_wrapper_refuses_oauth_overwrite_argv_leaks_and_wrong_provider(self):
        _, envelope = fixture("openai")
        payload = json.dumps(envelope).encode()
        token = envelope["credential"]["access_token"]
        expected = {"openai": token_hashes(payload), "claude": ["different"]}
        for argv in (["login", "--provider", "openai"],
                     ["auth", "import", "--provider", "openai", "--stdin", "--json", "--overwrite"],
                     ["auth", "import", "--provider", "openai", "--token", token],
                     ["auth", "import", "--provider", "claude", "--stdin", "--json"]):
            code, records, calls = self.exercise_wrapper(argv, payload, expected, None)
            self.assertEqual((code, calls), (93, 0))
            self.assertEqual(records, [{"operation": "guard_refusal"}])

    def test_wrapper_detects_secret_cli_output_without_forwarding(self):
        _, envelope = fixture("openai")
        payload = json.dumps(envelope).encode()
        result = subprocess.CompletedProcess([], 1, b"", envelope["credential"]["access_token"].encode())
        code, records, calls = self.exercise_wrapper(
            ["auth", "import", "--provider", "openai", "--stdin", "--json"],
            payload, {"openai": token_hashes(payload)}, result)
        self.assertEqual((code, calls), (94, 1))
        self.assertTrue(records[0]["output_leak"])

    def test_command_gate_forbids_prompts_and_oauth(self):
        tui = object.__new__(ImportPTY)
        for command in ("hello", "/login openai", "/login claude", "/login --import-local google"):
            with self.assertRaises(AssertionError):
                tui.command(command)

    def test_picker_navigation_uses_terminal_keys_and_never_confirms_copy(self):
        for provider in FILES:
            for imports_only in (False, True):
                tui = object.__new__(ImportPTY)
                tui.master, tui.output = 123, bytearray()
                tui.config = {"HOST": "test-remote"}
                tui.pump, tui.wait = mock.Mock(), mock.Mock()
                with mock.patch("os.write") as write:
                    tui.choose_import(provider, imports_only)
                expected = [b"/login --import-local\r" if imports_only else b"/login\r"]
                if imports_only:
                    expected.append(b"\x1b[200~" + provider.encode() + b"\x1b[201~")
                else:
                    expected.append(b"\x1b[200~import local\x1b[201~")
                    if provider == "claude":
                        expected.append(b"\x1b[B")
                expected.append(b"\r")
                self.assertEqual([call.args for call in write.call_args_list],
                                 [(123, value) for value in expected])
                self.assertEqual(tui.wait.call_count, 3)
                self.assertNotIn(b"confirm", b"".join(expected))
        tui = object.__new__(ImportPTY)
        with mock.patch("os.write") as write, self.assertRaises(AssertionError):
            tui.choose_import("google")
        write.assert_not_called()

    def test_arrow_decline_never_submits_highlighted_yes(self):
        tui = object.__new__(ImportPTY)
        tui.master, tui.output, tui.pump = 123, bytearray(), mock.Mock()
        with mock.patch("os.write") as write:
            tui.decline_with_arrows()
        self.assertEqual([call.args for call in write.call_args_list],
                         [(123, b"\x1b[A"), (123, b"\x1b[B\r")])

    def test_snapshot_detects_cancel_transfer_and_repeat_mutation(self):
        other = {"sha256": "other", "mode": 0o600, "tokens": ["other"]}
        baseline = {"stores": {"jcode/auth.json": other}, "calls": [], "starts": [],
                    "states": {"openai": "not_configured"}}
        assert_snapshot(baseline, "openai", baseline, {"openai": ["selected"]}, False, 0)
        bad = dict(baseline, stores={**baseline["stores"], "jcode/openai-auth.json": {}})
        with self.assertRaises(AssertionError):
            assert_snapshot(bad, "openai", baseline, {}, False, 0)
        bad = dict(baseline, starts=[{"provider": "openai"}])
        with self.assertRaises(AssertionError):
            assert_snapshot(bad, "openai", baseline, {}, False, 0)
        bad = dict(baseline, stores={"jcode/auth.json": {**other, "sha256": "changed"}})
        with self.assertRaises(AssertionError):
            assert_snapshot(bad, "openai", baseline, {}, False, 0)


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]], verbosity=2)
    else:
        try:
            configuration = configured()
            if configuration:
                run_acceptance(configuration)
        except Exception as error:
            # Never emit traceback/exception payloads from subprocess or JSON errors.
            print("FAIL native SSH import acceptance (" + type(error).__name__ + "); private output withheld", file=sys.stderr)
            sys.exit(1)
