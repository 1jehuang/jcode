#!/usr/bin/env python3
"""Accept safe runtime replacement against the real package binary, never the live daemon.

Usage: python3 scripts/runtime_upgrade_acceptance.py --binary target/selfdev/jcode
Requires Linux unshare user/network namespaces. Every daemon has private HOME,
JCODE_HOME, runtime/config/cache/state directories and a credential-free environment.
No model turn is requested. Context-only messages exercise actual persistence.
Artifacts and JSON wire evidence remain under JCODE_SCRATCH_DIR (or --artifacts).
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import tempfile
import time


class Wire:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(str(path))
        self.reader = self.socket.makefile("rb")
        self.events = []

    def close(self):
        self.reader.close()
        self.socket.close()

    def send(self, kind, request_id, **fields):
        self.socket.sendall((json.dumps({"type": kind, "id": request_id, **fields}) + "\n").encode())

    def until(self, kind, request_id=None):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            self.socket.settimeout(max(.01, deadline - time.monotonic()))
            line = self.reader.readline(1024 * 1024)
            if not line:
                raise EOFError(f"disconnected waiting for {kind}")
            event = json.loads(line)
            self.events.append(event)
            if event.get("type") == "error":
                raise AssertionError(f"native error: {event}")
            if event.get("type") == kind and (request_id is None or event.get("id") == request_id):
                return event
        raise TimeoutError(f"waiting for {kind}")


def wait_until(callback, timeout=20):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = callback()
            if result:
                return result
        except (OSError, EOFError, TimeoutError) as error:
            last = error
        time.sleep(.05)
    raise TimeoutError(f"readiness timeout: {last}")


class PrivateDaemon:
    def __init__(self, binary, root, version):
        self.root = root
        root.mkdir(mode=0o700)
        for name in ("home", "data", "runtime", "config", "cache", "state", "work", "install"):
            (root / name).mkdir(mode=0o700)
        (root / "data" / "no_telemetry").touch()
        self.path = root / "runtime" / "jcode.sock"
        self.debug = root / "runtime" / "jcode-debug.sock"
        self.env = {
            "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "SHELL": "/bin/bash",
            "HOME": str(root / "home"), "JCODE_HOME": str(root / "data"),
            "JCODE_RUNTIME_DIR": str(root / "runtime"), "JCODE_SOCKET": str(self.path),
            "XDG_RUNTIME_DIR": str(root / "runtime"), "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_CACHE_HOME": str(root / "cache"), "XDG_STATE_HOME": str(root / "state"),
            "XDG_DATA_HOME": str(root / "data"), "JCODE_INSTALL_DIR": str(root / "install"),
            "JCODE_NO_AUTO_UPDATE": "1", "JCODE_DEBUG_CONTROL": "1",
            "JCODE_MEMORY_ENABLED": "0", "JCODE_MEMORY_SIDECAR_ENABLED": "0",
            "JCODE_RUNTIME_RELEASE_SEMVER": version,
            "JCODE_NON_INTERACTIVE": "1",
        }
        self.log = (root / "daemon.log").open("wb")
        self.process = subprocess.Popen(
            ["/usr/bin/unshare", "--user", "--map-root-user", "--net", str(binary),
             "--no-update", "--socket", str(self.path), "serve"],
            env=self.env, cwd=root / "work", stdin=subprocess.DEVNULL,
            stdout=self.log, stderr=self.log, start_new_session=True,
        )
        try:
            wait_until(self.ping)
        except Exception:
            self.close()
            raise AssertionError((root / "daemon.log").read_text(errors="replace"))

    def close(self):
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        self.log.close()

    def ping(self):
        client = Wire(self.path)
        try:
            client.send("ping", 1)
            return client.until("pong", 1)
        finally:
            client.close()

    def info(self):
        client = Wire(self.debug)
        try:
            client.send("debug_command", 2, command="server:info")
            result = client.until("debug_response", 2)
            assert result["ok"], result
            return json.loads(result["output"])
        finally:
            client.close()

    def upgrade(self, candidate):
        client = Wire(self.path)
        try:
            client.send("upgrade_runtime", 11, candidate=str(candidate))
            result = client.until("reload_progress")
            if result["step"] == "upgrading":
                assert client.reader.readline(1024 * 1024) == b"", "exec must close old connection"
            return result
        finally:
            client.close()


def run(binary, artifacts):
    binary = binary.resolve(strict=True)
    assert binary.is_file() and os.access(binary, os.X_OK)
    subprocess.run(["/usr/bin/unshare", "--user", "--map-root-user", "--net", "/bin/true"], check=True)
    evidence = {"binary": str(binary), "sha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest()}
    parent_network = os.readlink("/proc/self/ns/net")
    daemon = PrivateDaemon(binary, artifacts / "upgrade", "0.0.1")
    client = None
    try:
        before = daemon.info()
        evidence["before"] = before
        client = Wire(daemon.path)
        client.send("subscribe", 30, working_dir=str(daemon.root / "work"))
        client.send("input_shell", 3, command="readlink /proc/self/ns/net")
        namespace = client.until("input_shell_result")["result"]
        client.until("done", 3)
        assert namespace["exit_code"] == 0, namespace
        assert namespace["output"].strip() != parent_network, "daemon must have no host network namespace"
        evidence["isolated_network"] = namespace["output"].strip()
        client.send("message", 4, content="upgrade acceptance: preserved context A", no_reply=True)
        client.until("context_message_added", 4)
        client.send("message", 5, content="upgrade acceptance: preserved context B", no_reply=True)
        client.until("context_message_added", 5)
        client.send("get_history", 6)
        history = client.until("history", 6)
        evidence["session_id"] = history["session_id"]
        evidence["history_before"] = history["messages"]
        assert "preserved context A" in json.dumps(history["messages"]), history
        assert "preserved context B" in json.dumps(history["messages"]), history
        invalid = artifacts / "not-a-native-binary"
        invalid.write_text("#!/bin/sh\nexit 0\n")
        invalid.chmod(0o700)
        native_nonjcode = artifacts / "native-nonjcode"
        shutil.copyfile("/usr/bin/true", native_nonjcode)
        native_nonjcode.chmod(0o700)
        invalid_results = []
        for candidate in (Path("relative-candidate"), artifacts / "missing-binary", invalid, native_nonjcode):
            result = daemon.upgrade(candidate)
            assert result["step"] == "invalid", result
            assert daemon.info()["id"] == before["id"], "invalid candidate replaced daemon"
            invalid_results.append(result)
        assert "not a jcode version report" in invalid_results[-1]["message"], invalid_results[-1]
        evidence["invalid_refusals"] = invalid_results
        marker = daemon.root / "shell-started"
        client.send("input_shell", 7, command=f"printf started > '{marker}'; sleep 3; printf uninterrupted-shell-ok")
        wait_until(marker.exists)
        result = daemon.upgrade(binary)
        assert result["step"] == "busy", result
        assert daemon.info()["id"] == before["id"], "busy upgrade replaced daemon"
        evidence["busy_refusal"] = result
        shell = client.until("input_shell_result")["result"]
        client.until("done", 7)
        assert shell["exit_code"] == 0 and shell["output"] == "uninterrupted-shell-ok", shell
        evidence["uninterrupted_shell"] = shell
        client.send("get_history", 8)
        assert client.until("history", 8)["messages"] == history["messages"], "refusals mutated history"
        result = wait_until(lambda: (reply if (reply := daemon.upgrade(binary))["step"] != "busy" else None))
        assert result["step"] == "upgrading", result
        evidence["upgrade"] = result
        assert client.reader.readline(1024 * 1024) == b"", "attached session was not disconnected by exec"
        client.close()
        client = None
        after = wait_until(lambda: (info if (info := daemon.info())["id"] != before["id"] else None))
        evidence["after"] = after
        assert daemon.process.poll() is None, "daemon exited instead of replacing itself"
        assert daemon.upgrade(binary)["step"] == "unchanged", "replacement version was not confirmed"
        client = Wire(daemon.path)
        client.send("subscribe", 31, working_dir=str(daemon.root / "work"), target_session_id=history["session_id"])
        client.send("get_history", 9)
        restored = client.until("history", 9)
        assert restored["session_id"] == history["session_id"]
        assert restored["messages"] == history["messages"], "history was lost, duplicated, or changed across exec"
        evidence["history_after"] = restored["messages"]
        evidence["pid_preserved_across_exec"] = daemon.process.pid
    finally:
        if client:
            client.close()
        daemon.close()
    newer = PrivateDaemon(binary, artifacts / "downgrade", "999.0.0")
    try:
        before = newer.info()
        result = newer.upgrade(binary)
        assert result["step"] == "unchanged", result
        assert newer.info()["id"] == before["id"], "downgrade replaced newer daemon"
        evidence["downgrade_refusal"] = result
    finally:
        newer.close()
    (artifacts / "acceptance.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({"result": "passed", "artifacts": str(artifacts), "session": evidence["session_id"],
                      "checks": ["private network namespace", "credential-free environment", "invalid/native-nonjcode refusal",
                                 "busy refusal", "uninterrupted shell", "real exec/reconnect", "history exact preservation",
                                 "same-version refusal", "downgrade refusal"]}, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path)
    args = parser.parse_args()
    if args.artifacts:
        artifacts = args.artifacts.resolve()
        artifacts.mkdir(mode=0o700, parents=True, exist_ok=False)
    else:
        scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", Path.home() / ".jcode" / "scratch"))
        scratch.mkdir(parents=True, exist_ok=True)
        artifacts = Path(tempfile.mkdtemp(prefix="upg-", dir=scratch))
    assert len(str(artifacts / "downgrade" / "runtime" / "jcode-debug.sock").encode()) < 104, "artifact root too long for Unix socket"
    run(args.binary, artifacts)


if __name__ == "__main__":
    main()
