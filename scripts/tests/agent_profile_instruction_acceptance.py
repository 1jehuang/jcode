#!/usr/bin/env python3
"""Public binary acceptance for isolated session instruction profiles, stdlib only.

Run only after the coordinator grants a runtime slot:
  python3 scripts/tests/agent_profile_instruction_acceptance.py --binary /abs/jcode --execute
All requests use a private Unix socket and a loopback fixture provider. No credentials,
shared daemon, real inference, tools, updates, telemetry, or installed symlinks are used.
"""
import argparse
from contextlib import suppress
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time

def require(condition, message):
    if not condition:
        raise AssertionError(message)

class Peer:
    def __init__(self, fixture):
        self.fixture, self.counter, self.buffer = fixture, 100, b""
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        fixture.peers.append(self)
        self.socket.settimeout(10)
        self.socket.connect(str(fixture.endpoint))

    def send(self, kind, **fields):
        self.counter += 1
        request = dict(type=kind, id=self.counter, **fields)
        self.fixture.wire.write(json.dumps(dict(send=request)) + "\n")
        self.fixture.wire.flush()
        self.socket.sendall((json.dumps(request) + "\n").encode())
        return self.counter

    def until(self, request_id, response, reject=False):
        while time.monotonic() < self.fixture.deadline:
            while b"\n" not in self.buffer:
                chunk = self.socket.recv(65536)
                require(chunk, "private server disconnected")
                self.buffer += chunk
                require(len(self.buffer) < 16 * 1024 * 1024, "protocol buffer exceeded bound")
            raw, self.buffer = self.buffer.split(b"\n", 1)
            event = json.loads(raw)
            self.fixture.wire.write(json.dumps(dict(receive=event)) + "\n")
            self.fixture.wire.flush()
            if event.get("id") == request_id and event["type"] in (response, "error"):
                require((event["type"] == "error") == reject, f"unexpected response: {event}")
                return event
        raise TimeoutError(f"waiting for {response}")

    def request(self, kind, response="done", reject=False, **fields):
        return self.until(self.send(kind, **fields), response, reject)

    def agent(self, *args, reject=False):
        result = self.request("runtime_command", "runtime_command_result", reject,
                              command="agent", args=list(args))
        return result if reject else result["data"]

    def subscribe(self, target=None, name=None, system_prompt=None):
        fields = dict(working_dir=str(self.fixture.cwd), continue_on_disconnect=False)
        if target: fields["target_session_id"] = target
        if name: fields["agent_profile_name"] = name
        if system_prompt: fields["system_prompt"] = system_prompt
        self.request("subscribe", **fields)
        return self.request("state", "state")["session_id"]

class Fixture:
    def __init__(self, binary, seconds=150):
        self.binary, self.deadline = binary, time.monotonic() + seconds
        self.server = self.http = self.http_thread = self.wire = self.log = None
        self.peers, self.release = [], threading.Event()
        self.release.set()
        try:
            self._start()
        except BaseException as error:
            with suppress(BaseException):
                cleanup = self.close()
                if hasattr(self, "root"):
                    (self.root / "startup-failure.json").write_text(json.dumps({"error": repr(error), "cleanup": cleanup}))
            raise

    def _start(self):
        binary = self.binary
        scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", Path.home() / ".jcode/scratch"))
        self.root = Path(tempfile.mkdtemp(prefix="jpi-", dir=scratch))
        self.home, self.jhome, self.runtime, self.cwd = [self.root / n for n in ("h", "j", "r", "w")]
        for path in (self.home, self.jhome, self.runtime, self.cwd, self.root / "cache", self.root / "tmp"):
            path.mkdir(mode=0o700)
        self.endpoint = self.runtime / "s.sock"
        require(len(os.fsencode(self.runtime / "s-debug.sock")) < 104, "private socket path too long")
        self.sources, self.markers, self.peers, self.captures = {}, {}, [], []
        locations = {"project_agents": self.cwd / "AGENTS.md", "global_agents": self.jhome / "external/AGENTS.md"}
        for scope, directory in (("project", self.cwd / ".jcode"), ("global", self.jhome)):
            for name in ("system-prompt", "prompt-overlay", "preferred-tools", "swarm-prompt"):
                locations[f"{scope}_{name}"] = directory / f"{name}.md"
        for key, path in locations.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            self.markers[key] = "SENTINEL_INSTRUCTION_" + key.replace("-", "_").upper() + "_798321"
            path.write_text(self.markers[key] + "\n" + "fixture instruction context. " * 200)
            self.sources[key] = "file:" + str(path.resolve())
        self.profile_marker = "SENTINEL_SELECTED_PROFILE_563492"
        profile = self.cwd / ".jcode/agents/reviewer.md"
        profile.parent.mkdir(parents=True)
        profile.write_text(self.profile_marker + "\n")
        self.profile_path = profile
        self.release, self.arrived = threading.Event(), threading.Event()
        self.release.set()
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_POST(self):
                raw = self.rfile.read(int(self.headers["Content-Length"]))
                data = json.loads(raw)
                require(self.path == "/v1/chat/completions" and data.get("stream") is True, "unexpected provider route or stream mode")
                require(self.headers.get("Authorization") is None, "fixture must never receive credentials")
                fixture.captures.append(data)
                (fixture.root / f"provider-{len(fixture.captures):03}.json").write_bytes(raw)
                fixture.arrived.set()
                require(fixture.release.wait(20), "provider hold exceeded bound")
                chunks = [{"choices": [{"index": 0, "delta": {"role": "assistant", "content": "Fixture acknowledged."}, "finish_reason": None}]},
                          {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}]
                body = "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks) + "data: [DONE]\n\n"
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(body.encode())))
                self.end_headers()
                self.wfile.write(body.encode())
        self.http = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.http.daemon_threads = True
        self.http_thread = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.http_thread.start()
        config = f'''[features]
check_updates = false
memory = false
swarm = false
[ambient]
enabled = false
[providers.acceptance]
type = "openai-compatible"
base_url = "http://127.0.0.1:{self.http.server_port}/v1"
auth = "none"
requires_api_key = false
model_catalog = false
provider_routing = false
allow_provider_pinning = false
default_model = "acceptance-no-inference"
[[providers.acceptance.models]]
id = "acceptance-no-inference"
'''
        (self.jhome / "config.toml").write_text(config)
        (self.jhome / "setup_hints.json").write_text('{"launch_count":20}')
        self.env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL") if k in os.environ}
        self.env.update(HOME=str(self.home), JCODE_HOME=str(self.jhome), JCODE_RUNTIME_DIR=str(self.runtime),
                        XDG_RUNTIME_DIR=str(self.runtime), XDG_CONFIG_HOME=str(self.home / ".config"),
                        XDG_CACHE_HOME=str(self.root / "cache"), TMPDIR=str(self.root / "tmp"),
                        JCODE_SOCKET=str(self.endpoint), JCODE_NO_TELEMETRY="1", DO_NOT_TRACK="1",
                        JCODE_NO_MENUBAR="1", TERM="xterm-256color")
        self.wire = (self.root / "wire.jsonl").open("w")
        self.log = (self.root / "server.log").open("w")
        self.command = [str(binary), "--no-update", "--no-selfdev", "--socket", str(self.endpoint),
                        "--provider-profile", "acceptance", "--model", "acceptance-no-inference", "serve",
                        "--temporary-server", "--owner-pid", str(os.getpid()), "--temp-idle-timeout-secs", "120"]
        self.server = subprocess.Popen(self.command, cwd=self.cwd, env=self.env, stdin=subprocess.DEVNULL,
                                       stdout=self.log, stderr=subprocess.STDOUT)
        while not self.endpoint.exists():
            require(self.server.poll() is None and time.monotonic() < self.deadline, "private daemon startup failed")
            time.sleep(0.05)

    def capture(self, peer, label):
        before = len(self.captures)
        peer.request("message", content="fixture-turn:" + label)
        require(len(self.captures) == before + 1, f"expected one provider request for {label}")
        data = self.captures[-1]
        require(any(tool.get("function", {}).get("name") == "swarm" for tool in data.get("tools", [])), "swarm tool must be exercised")
        return data

    def assert_payload(self, data, selected, extras=()):
        serialized = json.dumps(data)
        require(serialized.count(self.profile_marker) == int(selected), "profile must occur exactly once when selected")
        for key, marker in self.markers.items():
            require(serialized.count(marker) == int(key in extras), f"unexpected instruction inclusion/count: {key}")
        return len(json.dumps({"system": [m for m in data["messages"] if m["role"] in ("system", "developer")],
                               "tools": data.get("tools", [])}).encode())

    def saved(self, session):
        return json.loads((self.jhome / "sessions" / f"{session}.json").read_text())

    def close(self):
        signal.alarm(0)
        self.release.set()
        errors = []
        def close_one(label, action):
            try: action()
            except BaseException as error: errors.append(f"{label}: {error!r}")
        for peer in self.peers: close_one("peer", peer.socket.close)
        if self.server is not None:
            def reap():
                if self.server.poll() is None: self.server.terminate()
                try: self.server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.server.kill()
                    self.server.wait(timeout=5)
            close_one("server", reap)
        if self.http is not None:
            if self.http_thread is not None and self.http_thread.is_alive():
                close_one("http shutdown", self.http.shutdown)
                close_one("http join", lambda: self.http_thread.join(timeout=2))
            close_one("http socket", self.http.server_close)
        for handle in (self.wire, self.log):
            if handle is not None: close_one("log", handle.close)
        return {"pid": self.server.pid if self.server else None,
                "returncode": self.server.returncode if self.server else None,
                "reaped": self.server is None or self.server.poll() is not None,
                "http_stopped": self.http_thread is None or not self.http_thread.is_alive(), "errors": errors}

def acceptance(f):
    report = {"status": "running", "checks": [], "root": str(f.root), "binary": str(f.binary)}
    def check(label):
        report["checks"].append(label)
        print("PASS " + label, flush=True)
    try:
        normal = Peer(f)
        normal_id = normal.subscribe()
        require(normal.agent()["current"] is None, "profiles must not auto-select")
        active = {"project_agents", "global_agents", "project_system-prompt", "project_prompt-overlay",
                  "global_prompt-overlay", "project_preferred-tools", "global_preferred-tools", "project_swarm-prompt"}
        report["normal_system_tool_bytes"] = f.assert_payload(f.capture(normal, "normal"), False, active)
        check("no profile preserves normal auto-loaded instruction policy")
        p = Peer(f)
        sid = p.subscribe(name="reviewer")
        initial = p.agent()
        require(initial["included_sources"] == [] and not any(s["included"] for s in initial["instruction_sources"]), "subscribe profile must default to no extras")
        require(set(f.sources.values()) <= {s["id"] for s in initial["instruction_sources"]}, "source discovery missing fixture files")
        report["profile_only_system_tool_bytes"] = f.assert_payload(f.capture(p, "isolated"), True)
        require(report["profile_only_system_tool_bytes"] < report["normal_system_tool_bytes"], "fixture context must shrink")
        check("selected profile occurs once; excluded sentinels absent from actual system and tools")
        extras = ["project_agents", "global_prompt-overlay", "project_swarm-prompt"]
        ids = [f.sources[key] for key in extras]
        p.agent("--select", "reviewer", json.dumps(ids))
        f.assert_payload(f.capture(p, "extras"), True, extras)
        before = f.saved(sid)["agent_profile"]
        p.agent("--select", "reviewer", '["file:/not-an-available-source"]', reject=True)
        require(f.saved(sid)["agent_profile"] == before, "invalid apply must be atomic")
        check("explicit extras only are included; invalid selection leaves saved snapshot unchanged")
        other = Peer(f)
        other.subscribe(target=sid)
        f.profile_path.with_name("editor.md").write_text("SENTINEL_SECOND_PROFILE_49173\n")
        other.agent("editor")
        stale_before = f.saved(sid)["agent_profile"]
        p.agent("--sources", "reviewer", json.dumps(ids), reject=True)
        require(f.saved(sid)["agent_profile"] == stale_before, "stale Apply changed newer profile")
        p.agent("--select", "reviewer", json.dumps(ids))
        check("cross-client stale draft rejects atomically without changing the newer profile")
        retained_path = Path(ids[0].removeprefix("file:"))
        retained_body = retained_path.read_text()
        retained_path.unlink()
        f.profile_path.write_text(f.profile_marker + "\nReloaded fixture body.\n")
        p.agent("reload")
        require(f.saved(sid)["agent_profile"]["instruction_sources"] == before["instruction_sources"], "reload must retain extras")
        child = p.request("split", "split_response")["new_session_id"]
        for target in (sid, child):
            resumed = Peer(f)
            resumed.subscribe(target=target)
            f.assert_payload(f.capture(resumed, "resume-" + target), True, extras)
        retained_path.write_text(retained_body)
        check("resume and fork preserve instruction snapshots, including an unavailable chosen source")
        p.agent("--sources", "[]")
        f.assert_payload(f.capture(p, "sources-off"), True)
        f.release.clear()
        f.arrived.clear()
        message_id = p.send("message", content="fixture-turn:busy")
        require(f.arrived.wait(10), "busy turn did not reach actual provider")
        try:
            for args in (("off",), ("reviewer",), ("reload",), ("--sources", "reviewer", "[]")):
                p.agent(*args, reject=True)
        finally:
            f.release.set()
        p.until(message_id, "done")
        require(p.agent()["current"] == "reviewer", "busy mutation must not apply")
        check("busy profile changes reject without mutation during actual provider request")
        p.agent("off")
        f.assert_payload(f.capture(p, "restored"), False, active)
        require(normal.agent()["current"] is None and f.saved(normal_id).get("agent_profile") is None, "other session changed")
        check("off restores normal policy without changing another session or a default")
        custom = Peer(f)
        custom_marker = "SENTINEL_SESSION_OVERRIDE_13579"
        custom.subscribe(system_prompt=custom_marker)
        custom.agent("reviewer")
        isolated = f.capture(custom, "custom-excluded")
        f.assert_payload(isolated, True)
        require(custom_marker not in json.dumps(isolated), "session custom prompt leaked")
        custom.agent("--sources", "reviewer", '["session:system_prompt"]')
        opted = f.capture(custom, "custom-opted-in")
        f.assert_payload(opted, True)
        require(json.dumps(opted).count(custom_marker) == 1, "session custom prompt opt-in missing or duplicated")
        custom.agent("off")
        require(custom_marker in json.dumps(f.capture(custom, "custom-restored")), "off lost custom override")
        check("custom session prompt is excluded, explicitly selectable, and restored by off")
        report["status"] = "passed"
    except BaseException as error:
        report.update(status="failed", error=repr(error))
        raise
    finally:
        passed_before_cleanup = report["status"] == "passed"
        report["cleanup"] = f.close()
        cleanup_ok = report["cleanup"]["reaped"] and report["cleanup"]["http_stopped"] and not report["cleanup"]["errors"]
        if not cleanup_ok: report["status"] = "failed"
        (f.root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Artifacts: " + str(f.root), flush=True)
        if passed_before_cleanup: require(cleanup_ok, "private resource cleanup failed")

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    if not args.execute:
        print("Prepared only. Runtime requires coordinator slot and --execute.")
        return
    require(args.binary.is_absolute() and args.binary.is_file() and not args.binary.is_symlink(), "explicit built binary required")
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError("global acceptance deadline")))
    signal.alarm(160)
    acceptance(Fixture(args.binary))
    signal.alarm(0)

if __name__ == "__main__":
    main()
