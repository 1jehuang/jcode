"""Real SSH + real terminal acceptance scenarios, with disposable local/cloud disks."""
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import time

SCRIPTS = Path(__file__).resolve().parent


def run(args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True,
                          timeout=kwargs.pop("timeout", 20), **kwargs)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(predicate, description, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}")


class CloudTUI:
    """No current session, real credentials, shared daemon, or paid VM is touched."""

    def __init__(self, binary, root):
        self.binary = str(Path(binary).resolve())
        self.root = Path(root).resolve()
        self.root.mkdir(parents=True)
        # Unix sockets have a 108-byte path limit. Keep ephemeral sockets out of
        # arbitrarily deep artifact paths, while retaining all logs in root.
        self.runtime = tempfile.TemporaryDirectory(prefix="jca-", dir="/tmp")
        for side in ("local", "cloud"):
            (Path(self.runtime.name) / side).mkdir()
        self.children = []
        self.files = []
        self.env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TERM": "xterm-256color",
                    "HOME": str(self.root / "local-home"), "GIT_CONFIG_NOSYSTEM": "1",
                    "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0",
                    "JCODE_TEST_WORKSPACE": str(self.root)}
        self.repo = self.root / "local-repo"
        self.cloud_repo = self.root / "cloud-repo"
        self.control = self.root / "control"
        self.sock = str(Path(self.runtime.name) / "tmux.sock")
        self.sid = None
        for name in ("local-repo", "cloud-repo", "local-home", "cloud-home",
                     "control", "ssh", "bin"):
            (self.root / name).mkdir()

    def checkpoint(self, message):
        print("JCODE_CHECKPOINT " + json.dumps({"message": message, "scenario": self.root.name}), flush=True)

    def spawn(self, args, logfile, env=None):
        handle = (self.root / logfile).open("w")
        self.files.append(handle)
        child = subprocess.Popen(args, env=env or self.env, stdout=handle,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        self.children.append(child)
        return child

    def tmux(self, *args):
        return run(["tmux", "-S", self.sock, *args], env=self.env).stdout

    def frame(self):
        text = self.tmux("capture-pane", "-p", "-S", "-1000", "-t", "acceptance")
        (self.root / "frame.txt").write_text(text)
        return text

    def wait_text(self, text, timeout=45):
        def visible():
            frame = self.frame()
            if "JCODE_ACCEPTANCE_EXIT=" in frame:
                raise AssertionError("TUI exited unexpectedly; see frame.txt")
            return text in frame
        result = wait_for(visible, repr(text), timeout)
        safe = "".join(c if c.isalnum() else "_" for c in text)[:50]
        (self.root / f"frame-{safe}.txt").write_text(self.frame())
        return result

    def send(self, text, submit=True):
        self.tmux("send-keys", "-t", "acceptance", "-l", text)
        if submit:
            self.tmux("send-keys", "-t", "acceptance", "Enter")

    def records(self):
        try:
            # The server appends one complete JSON line under a lock. Ignore an
            # in-progress final line rather than race the writer.
            lines = (self.root / "model.jsonl").read_text().splitlines()
            return [record for line in lines if line.endswith("}")
                    if (record := json.loads(line)).get("agent")]
        except FileNotFoundError:
            return []

    def wait_request(self, needle, start=0):
        return wait_for(lambda: next((r for r in self.records()[start:]
                                     if needle in json.dumps(r["messages"])), None),
                        f"provider context containing {needle!r}")

    def lease(self):
        if self.sid is None:
            return {}
        try:
            return json.loads((self.root / "local-home/session_leases" / (self.sid + ".json")).read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            return {}

    def discover_session(self):
        def find():
            for path in (self.root / "local-home/sessions").glob("*.json"):
                try:
                    data = json.loads(path.read_text())
                    if data.get("working_dir") == str(self.repo) and data.get("messages"):
                        return data["id"]
                except json.JSONDecodeError:
                    pass
        self.sid = wait_for(find, "persisted source session")
        return self.sid

    def session(self, cloud=False):
        home = "cloud-home" if cloud else "local-home"
        return json.loads((self.root / home / "sessions" / (self.sid + ".json")).read_text())

    def start(self):
        for repo in (self.repo,):
            run(["git", "init", "-q", "-b", "main", str(repo)], env=self.env)
            run(["git", "-C", str(repo), "config", "user.name", "Cloud acceptance fixture"], env=self.env)
            run(["git", "-C", str(repo), "config", "user.email", "fixture@example.invalid"], env=self.env)
            (repo / "file.txt").write_text("base\n")
            run(["git", "-C", str(repo), "add", "."], env=self.env)
            run(["git", "-C", str(repo), "commit", "-qm", "fixture"], env=self.env)
        (self.repo / "file.txt").write_text("base\nlocal uncommitted edit\n")
        provider_env = dict(self.env, FAKE_LOG=str(self.root / "model.jsonl"),
                            FAKE_CONTROL_DIR=str(self.control),
                            FAKE_PORT_FILE=str(self.root / "model.port"))
        self.spawn([sys.executable, str(SCRIPTS / "fake_openai_server.py"), "0"],
                   "provider.log", provider_env)
        port = wait_for(lambda: (self.root / "model.port").read_text()
                        if (self.root / "model.port").exists() else None, "provider readiness")
        model_env = {
            "JCODE_OPENAI_COMPAT_API_BASE": f"http://127.0.0.1:{port}/v1",
            "OPENAI_COMPAT_API_KEY": "synthetic-acceptance-key",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL": "fake-model",
            "JCODE_NO_TELEMETRY": "1", "JCODE_TELEMETRY": "off", "JCODE_NO_AUTO_UPDATE": "1",
        }
        for key in ("host_key", "client_key"):
            run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(self.root / "ssh" / key)], env=self.env)
        (self.root / "ssh/authorized_keys").write_bytes((self.root / "ssh/client_key.pub").read_bytes())
        ssh_port = free_port()
        host_key = " ".join((self.root / "ssh/host_key.pub").read_text().split()[:2])
        (self.root / "ssh/known_hosts").write_text(f"[127.0.0.1]:{ssh_port} {host_key}\n")
        cloud_env = dict(self.env, **model_env, HOME=str(self.root / "cloud-home"),
                         JCODE_HOME=str(self.root / "cloud-home"),
                         JCODE_RUNTIME_DIR=str(Path(self.runtime.name) / "cloud"),
                         JCODE_CLOUD_HOST_LABEL="cloud-acceptance")
        # Same absolute workspace path on separate disks. This is a real SSH
        # command, not the JCODE_CLOUD_TRANSPORT override used by CLI tests.
        mount_command = (f"mount --bind {shlex.quote(str(self.cloud_repo))} {shlex.quote(str(self.repo))}; "
                         'exec /bin/bash -c "$SSH_ORIGINAL_COMMAND"')
        shell = self.root / "cloud-shell"
        env_args = " ".join(shlex.quote(f"{k}={v}") for k, v in cloud_env.items())
        shell.write_text("#!/bin/bash\nset -e\n"
                         f"if [[ -e {shlex.quote(str(self.control / 'ssh-fail'))} ]]; then "
                         "echo 'Injected acceptance SSH outage' >&2; exit 43; fi\n"
                         f"exec env -i {env_args} \"SSH_ORIGINAL_COMMAND=$SSH_ORIGINAL_COMMAND\" "
                         f"unshare --user --map-root-user --mount /bin/bash -ec {shlex.quote(mount_command)}\n")
        shell.chmod(0o700)
        (self.root / "ssh/sshd_config").write_text(f"""Port {ssh_port}
ListenAddress 127.0.0.1
HostKey {self.root}/ssh/host_key
PidFile {self.root}/ssh/sshd.pid
AuthorizedKeysFile {self.root}/ssh/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
ForceCommand {shell}
LogLevel VERBOSE
""")
        self.spawn(["/usr/bin/sshd", "-D", "-e", "-f", str(self.root / "ssh/sshd_config")], "sshd.log")
        import pwd
        (self.root / "ssh/config").write_text(f"""Host cloud-acceptance
 HostName 127.0.0.1
 Port {ssh_port}
 User {pwd.getpwuid(os.getuid()).pw_name}
 IdentityFile {self.root}/ssh/client_key
 IdentitiesOnly yes
 StrictHostKeyChecking yes
 UserKnownHostsFile {self.root}/ssh/known_hosts
 ConnectTimeout 3
 BatchMode yes
""")
        ssh = self.root / "bin/ssh"
        ssh.write_text(f'#!/bin/bash\nexec /usr/bin/ssh -F {shlex.quote(str(self.root / "ssh/config"))} "$@"\n')
        ssh.chmod(0o700)
        remote_binary = self.root / "bin/remote-jcode"
        remote_binary.write_text(
            "#!/bin/bash\n"
            f"if [[ -e {shlex.quote(str(self.control / 'binary-missing'))} ]]; then "
            "echo 'injected missing destination binary' >&2; exit 127; fi\n"
            f"if [[ -e {shlex.quote(str(self.control / 'binary-legacy'))} ]]; then\n"
            "  if [[ \"$*\" == *--version* ]]; then echo 'jcode legacy acceptance fixture'; exit 0; fi\n"
            "  echo 'unrecognized cloud protocol command' >&2; exit 2\nfi\n"
            f"exec {shlex.quote(self.binary)} \"$@\"\n")
        remote_binary.chmod(0o700)
        self.local_env = dict(self.env, **model_env, HOME=str(self.root / "local-home"),
                              PATH=f"{self.root}/bin:/usr/bin:/bin",
                              JCODE_HOME=str(self.root / "local-home"),
                              JCODE_RUNTIME_DIR=str(Path(self.runtime.name) / "local"),
                              JCODE_CLOUD_HOST="cloud-acceptance", JCODE_CLOUD_HOST_LABEL="laptop-acceptance",
                              JCODE_CLOUD_REMOTE_BINARY=str(remote_binary),
                              JCODE_DEBUG_CMD_PATH=str(self.control / "debug.cmd"),
                              JCODE_DEBUG_RESPONSE_PATH=str(self.control / "debug.response"))
        def probe():
            return subprocess.run([str(ssh), "cloud-acceptance", "printf ready"], env=self.local_env,
                                  capture_output=True, timeout=5).returncode == 0
        wait_for(probe, "isolated real SSH host", 15)
        self.checkpoint("Isolated SSH host is ready")
        args = [self.binary, "--no-update", "--no-selfdev", "--provider", "openai-compatible", "--model", "fake-model"]
        command = f"cd {shlex.quote(str(self.repo))} && env -i "
        command += " ".join(shlex.quote(f"{k}={v}") for k, v in self.local_env.items()) + " " + shlex.join(args)
        command += '; result=$?; printf "\\nJCODE_ACCEPTANCE_EXIT=%s\\n" "$result"; sleep 600'
        self.tmux("-f", "/dev/null", "new-session", "-d", "-s", "acceptance", "-x", "160", "-y", "48",
                  shlex.join(["/bin/bash", "-c", command]))
        def ready():
            frame = self.frame()
            if "JCODE_ACCEPTANCE_EXIT=" in frame:
                raise AssertionError("TUI exited during startup; see frame.txt")
            return "Welcome to jcode" in frame or "Ready to use" in frame
        wait_for(ready, "TUI startup")
        if "Welcome to jcode" in self.frame():
            self.tmux("send-keys", "-t", "acceptance", "Right", "Enter")
        self.wait_text("Ready to use")
        self.checkpoint("Real TUI is ready")

    def state(self):
        # Read-only observation of the real client. Inputs still travel through
        # tmux/terminal events; no internal state is seeded or patched here.
        response = self.control / "debug.response"
        response.unlink(missing_ok=True)
        temporary = self.control / "debug.next"
        temporary.write_text("state")
        temporary.replace(self.control / "debug.cmd")
        def received():
            try:
                return json.loads(response.read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                return None
        state = wait_for(received, "read-only client state", 10)
        (self.root / "state.json").write_text(json.dumps(state, indent=2))
        return state

    def wait_idle(self):
        wait_for(lambda: (not (state := self.state())["processing"]
                          and state["queued_messages"] == 0), "idle client")

    def release(self, token):
        (self.control / (token + ".release")).touch()

    def held_turn(self, token, todo=False):
        self.send(("[e2e:todo] " if todo else "") + f"[e2e:hold:{token}] continue this task")
        wait_for(lambda: (self.control / (token + ".held")).exists(), "held provider turn")
        self.discover_session()

    def prepare_while_held(self):
        self.send("/cloud")
        self.wait_text("destination ready")
        assert not self.lease().get("away_host"), "source ownership changed during an active turn"
        self.checkpoint("Destination prepared while source turn remains active")

    def await_cloud(self, start):
        def transferred():
            if "Pending cloud handoff canceled" in self.frame():
                raise AssertionError("TUI canceled the prepared handoff: see frame.txt")
            return self.lease().get("away_host") == "cloud-acceptance"
        wait_for(transferred, "cloud ownership")
        def cloud_continuation():
            for record in self.records()[start:]:
                last_user = next((m for m in reversed(record["messages"]) if m.get("role") == "user"), {})
                if "[jcode cloud]" in str(last_user.get("content", "")):
                    return record
        wait_for(cloud_continuation, "new cloud continuation request")
        self.wait_text("saw_cloud_notice=True")
        self.wait_idle()
        assert "Pending cloud handoff canceled" not in self.frame()
        assert "local uncommitted edit" in (self.cloud_repo / "file.txt").read_text()
        self.checkpoint("Destination agent continued with transferred workspace")

    def go_cloud(self):
        self.wait_idle()
        start = len(self.records())
        self.send("/cloud")
        self.await_cloud(start)

    def go_local(self):
        start = len(self.records())
        self.send("/local")
        wait_for(lambda: self.lease() and not self.lease().get("away_host"), "local ownership")
        def local_ready():
            frame = self.frame()
            return "server:" in frame and "server: SSH" not in frame and "OpenAI-compatible Fake Model" in frame
        wait_for(local_ready, "local TUI reattachment")
        self.wait_idle()
        # A new prompt proves the reattached local runtime actually runs, not
        # just that a success message was printed or the lease was changed.
        self.send("acceptance back on laptop")
        self.wait_request("acceptance back on laptop", start)
        self.wait_idle()
        self.wait_text("saw_cloud_notice=True")
        assert "returned from cloud host" in json.dumps(self.session())
        self.checkpoint("Source agent answered a fresh prompt after return")

    def close(self):
        try:
            self.frame()
        except (subprocess.SubprocessError, OSError):
            pass
        subprocess.run(["tmux", "-S", self.sock, "kill-server"], env=self.env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        for side in ("local", "cloud"):
            env = dict(self.env, HOME=str(self.root / f"{side}-home"),
                       JCODE_HOME=str(self.root / f"{side}-home"),
                       JCODE_RUNTIME_DIR=str(Path(self.runtime.name) / side))
            try:
                subprocess.run([self.binary, "--no-update", "server", "stop"], env=env,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
            except subprocess.TimeoutExpired:
                pass
        for child in reversed(self.children):
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        # Exec-based reattachment can outlive the original tmux process. Reap
        # only processes carrying this fixture's unique workspace marker.
        marker = ("JCODE_TEST_WORKSPACE=" + str(self.root)).encode()
        def owned():
            pids = []
            for entry in Path("/proc").iterdir():
                if not entry.name.isdigit() or int(entry.name) == os.getpid():
                    continue
                try:
                    if marker in (entry / "environ").read_bytes().split(b"\0"):
                        pids.append(int(entry.name))
                except (OSError, ProcessLookupError):
                    pass
            return pids
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for pid in owned():
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
            deadline = time.monotonic() + 3
            while owned() and time.monotonic() < deadline:
                time.sleep(0.05)
        remaining = owned()
        if remaining:
            raise AssertionError(f"Fixture processes survived cleanup: {remaining}")
        for handle in self.files:
            handle.close()
        self.runtime.cleanup()


def idle_round_trip(t):
    t.send("acceptance original laptop prompt")
    t.wait_text("[model] ack")
    t.discover_session()
    for cycle in (1, 2):
        t.go_cloud()
        (t.cloud_repo / f"cloud-{cycle}.txt").write_text(f"cloud work {cycle}\n")
        (t.repo / f"local-{cycle}.txt").write_text(f"concurrent local work {cycle}\n")
        t.go_local()
        assert (t.repo / f"cloud-{cycle}.txt").read_text() == f"cloud work {cycle}\n"
        assert (t.repo / f"local-{cycle}.txt").exists()
        assert t.session().get("migration_epoch") == cycle * 2
    last = json.dumps(t.records()[-1]["messages"])
    assert "acceptance original laptop prompt" in last


def midturn_auto_poke(t):
    t.held_turn("poke", todo=True)
    todos = json.loads((t.root / "local-home/todos" / (t.sid + ".json")).read_text())
    assert any(todo["id"] == "acceptance-open" and todo["status"] == "in_progress" for todo in todos)
    t.prepare_while_held()
    start = len(t.records())
    t.release("poke")
    t.await_cloud(start)
    assert (t.root / "cloud-home/todos" / (t.sid + ".json")).exists()


def cancel_prepared(t):
    t.held_turn("cancel")
    t.prepare_while_held()
    t.send("/cloud cancel")
    t.wait_text("Pending cloud handoff canceled.")
    t.release("cancel")
    t.wait_text("[model] ack")
    t.send("still working locally after cancel")
    t.wait_request("still working locally after cancel")
    assert not t.lease().get("away_host")
    assert not t.session().get("migration_epoch", 0)


def draft_preserved(t):
    t.held_turn("draft")
    t.prepare_while_held()
    t.send("my unsent acceptance draft", submit=False)
    t.release("draft")
    t.wait_text("composer draft would be lost")
    assert not t.lease().get("away_host")
    t.tmux("send-keys", "-t", "acceptance", "Enter")
    t.wait_request("my unsent acceptance draft")
    assert not t.session().get("migration_epoch", 0)


def prepare_outage_retry(t):
    t.send("before SSH outage")
    t.wait_text("[model] ack")
    t.discover_session()
    (t.control / "ssh-fail").touch()
    t.send("/cloud")
    t.wait_text("Injected acceptance SSH outage")
    assert not t.lease().get("away_host")
    (t.control / "ssh-fail").unlink()
    t.go_cloud()


def transfer_outage_retry(t):
    t.held_turn("outage")
    t.prepare_while_held()
    (t.control / "ssh-fail").touch()
    t.release("outage")
    t.wait_text("Injected acceptance SSH outage")
    assert not t.lease().get("away_host")
    assert not t.session().get("migration_epoch", 0)
    (t.control / "ssh-fail").unlink()
    t.go_cloud()


def destination_binary_retry(t):
    t.send("check destination compatibility")
    t.wait_text("[model] ack")
    t.discover_session()
    t.wait_idle()
    for fault, expected in (("binary-missing", "could not start"), ("binary-legacy", "too old for cloud handoff")):
        marker = t.control / fault
        marker.touch()
        t.send("/cloud")
        t.wait_text(expected)
        assert not t.lease().get("away_host")
        assert not t.session().get("migration_epoch", 0)
        marker.unlink()
    t.go_cloud()


SCENARIOS = {
    "idle_round_trip": idle_round_trip,
    "midturn_auto_poke": midturn_auto_poke,
    "cancel_prepared": cancel_prepared,
    "draft_preserved": draft_preserved,
    "prepare_outage_retry": prepare_outage_retry,
    "transfer_outage_retry": transfer_outage_retry,
    "destination_binary_retry": destination_binary_retry,
}
