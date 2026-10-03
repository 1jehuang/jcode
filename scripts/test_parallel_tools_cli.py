#!/usr/bin/env python3
"""Exercise a built Jcode binary against loopback-only Responses and HTTP fixtures.

No real credentials, paid provider traffic, shared daemon, or user configuration.
Covers JSON/NDJSON opt-in policy, HTTP overlap, persisted timing, ordered replay,
native read/ls, unsafe barriers, bounded concurrency, and an HTTP error.
Run: python3 scripts/test_parallel_tools_cli.py --binary target/selfdev/jcode
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Fixture(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 32

    def __init__(self, make_calls, work):
        super().__init__(("127.0.0.1", 0), Handler)
        self.calls = make_calls(f"http://127.0.0.1:{self.server_port}")
        self.work = work
        self.lock = threading.Lock()
        self.requests = []
        self.request_times = []
        self.tool_response_sent = None
        self.http_events = []
        self.active_http = 0
        self.max_active_http = 0

    def snapshot(self):
        with self.lock:
            return json.loads(json.dumps({
                "requests": self.requests, "request_times": self.request_times,
                "tool_response_sent": self.tool_response_sent,
                "http_events": self.http_events, "active_http": self.active_http,
                "max_active_http": self.max_active_http,
            }))


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        fixture = self.server
        if self.path.startswith("/delay/"):
            _, _, token, delay, status = self.path.split("/")
            with fixture.lock:
                fixture.active_http += 1
                fixture.max_active_http = max(fixture.max_active_http, fixture.active_http)
                fixture.http_events.append({
                    "event": "start", "token": token, "time": time.monotonic(),
                    "active": fixture.active_http,
                    "barrier_exists": (fixture.work / "bash-marker").exists(),
                })
            time.sleep(float(delay))
            data = f"fixture-http-{token}".encode()
            # Finish bookkeeping before publishing bytes. A later tool cannot
            # race the handler's accounting after it receives this response.
            with fixture.lock:
                fixture.active_http -= 1
                fixture.http_events.append({"event": "end", "token": token,
                                            "time": time.monotonic(),
                                            "active": fixture.active_http,
                                            "barrier_exists": (fixture.work / "bash-marker").exists()})
            self.send_response(int(status))
            self.send_header("Content-Type", "text/plain")
        else:
            data = json.dumps({"object": "list", "data": [{"id": "gpt-5.4"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        fixture = self.server
        with fixture.lock:
            fixture.requests.append(body)
            fixture.request_times.append(time.monotonic())
            request_number = len(fixture.requests)
        first = request_number == 1
        events = []
        output = []
        if first:
            for index, (name, arguments) in enumerate(fixture.calls):
                item = {
                    "type": "function_call", "id": f"fc_{index}",
                    "call_id": f"call_{index}", "name": name,
                    "arguments": json.dumps(arguments), "status": "completed",
                }
                events.extend([
                    {"type": "response.output_item.added", "output_index": index,
                     "item": {**item, "arguments": "", "status": "in_progress"}},
                    {"type": "response.function_call_arguments.done", "item_id": item["id"],
                     "output_index": index, "arguments": item["arguments"]},
                    {"type": "response.output_item.done", "output_index": index, "item": item},
                ])
                output.append(item)
        else:
            output = [{"id": "msg_done", "type": "message", "role": "assistant",
                       "status": "completed", "content": [
                           {"type": "output_text", "text": "fixture complete", "annotations": []}]}]
            events.append({"type": "response.output_text.delta", "item_id": "msg_done",
                           "output_index": 0, "content_index": 0, "delta": "fixture complete"})
        events.append({"type": "response.completed", "response": {
            "id": f"resp_{request_number}", "status": "completed", "output": output,
            "usage": {"input_tokens": 100, "output_tokens": 20, "total_tokens": 120},
        }})
        data = "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        # Set before write/flush, not afterward: the next request may arrive
        # immediately on another server thread.
        if first:
            with fixture.lock:
                fixture.tool_response_sent = time.monotonic()
        self.wfile.write(data)
        self.wfile.flush()


def bash(command):
    return "bash", {"command": command, "intent": "unsafe barrier regression"}


def fetch(base, token, delay=0.7, status=200):
    return "webfetch", {"url": f"{base}/delay/{token}/{delay}/{status}",
                        "format": "text", "timeout": 10, "intent": "HTTP overlap regression"}


def overlap_calls(base):
    return [fetch(base, "slow0"), fetch(base, "fast", 0.08),
            fetch(base, "slow2"), fetch(base, "slow3")]


def barrier_calls(base):
    return [fetch(base, "before0", 0.25), fetch(base, "before1", 0.25),
            bash("printf barrier > bash-marker"),
            fetch(base, "after0", 0.25), fetch(base, "after1", 0.25),
            ("write", {"file_path": "sentinel", "content": "barrier-ok", "intent": "write barrier"}),
            ("read", {"file_path": "sentinel", "intent": "read after write"}),
            bash("sort --out=sorted input"),
            ("read", {"file_path": "sorted", "intent": "read after unsafe Bash"}),
            ("ls", {"path": ".", "intent": "native useful directory listing"})]


def run_case(binary, root, name, mode, config, override, enabled, make_calls,
             error_indices=()):
    case = root / name
    home = case / "home"
    work = case / "work"
    state = home / ".jcode"
    state.mkdir(parents=True)
    work.mkdir()
    (work / "input").write_text("z\na\n")
    if config is not None:
        (state / "config.toml").write_text(f"[tools]\nparallel = {str(config).lower()}\n")
    # No inherited credentials, proxies, session identity, or daemon socket.
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": str(home), "JCODE_HOME": str(state),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "JCODE_SCRATCH_DIR": str(case), "TMPDIR": str(case),
        "OPENAI_API_KEY": "sk-loopback-fixture-not-a-real-key",
        "JCODE_OPENAI_TRANSPORT": "https",
        "JCODE_TELEMETRY": "off", "JCODE_NO_TELEMETRY": "1",
        "JCODE_NO_AUTO_UPDATE": "1", "JCODE_RUN_AUTO_POKE": "0",
        "JCODE_HOOKS_DISABLED": "1", "NO_PROXY": "127.0.0.1,localhost",
        "TERM": "dumb",
    }
    if override is not None:
        env["JCODE_PARALLEL_TOOLS"] = str(override)
    fixture = Fixture(make_calls, work)
    calls = fixture.calls
    thread = threading.Thread(target=fixture.serve_forever, daemon=True)
    thread.start()
    env["JCODE_OPENAI_API_BASE"] = f"http://127.0.0.1:{fixture.server_port}/v1"
    command = [str(binary), "--no-update", "--no-selfdev", "--provider", "openai-api",
               "--model", "gpt-5.4", "--tools", "bash,read,write,ls,webfetch", "run", f"--{mode}",
               "Execute the fixture tool calls and finish."]
    try:
        result = subprocess.run(command, cwd=work, env=env, text=True,
                                capture_output=True, timeout=40)
    finally:
        fixture.shutdown()
        fixture.server_close()
        thread.join(timeout=5)
        snapshot = fixture.snapshot()
        (case / "observations.json").write_text(json.dumps(snapshot, indent=2))
        (case / "requests.json").write_text(json.dumps(snapshot["requests"], indent=2))
    (case / "stdout.txt").write_text(result.stdout)
    (case / "stderr.txt").write_text(result.stderr)
    assert result.returncode == 0, f"{name}: exit {result.returncode}: {result.stderr}"
    assert "fixture complete" in result.stdout, f"{name}: missing final answer"
    requests = snapshot["requests"]
    assert len(requests) == 2, f"{name}: expected two provider requests, got {len(requests)}"
    for request in requests:
        assert request.get("parallel_tool_calls") is enabled, (
            f"{name}: parallel_tool_calls={request.get('parallel_tool_calls')}, expected {enabled}")
    replay = [item for item in requests[1]["input"] if item.get("type") == "function_call_output"]
    expected = [f"call_{index}" for index in range(len(calls))]
    assert [item["call_id"] for item in replay] == expected, f"{name}: invalid result order/duplicates: {replay}"
    for index, item in enumerate(replay):
        text = str(item["output"])
        if index in error_indices:
            assert "HTTP error: 503" in text, f"{name}: missing HTTP error: {text}"
        else:
            assert "Error:" not in text and "HTTP error:" not in text, f"{name}: tool error: {text}"
            if calls[index][0] == "webfetch":
                token = calls[index][1]["url"].split("/")[-3]
                assert f"fixture-http-{token}" in text, f"{name}: missing HTTP body: {text}"
    records = ([json.loads(result.stdout)] if mode == "json" else
               [json.loads(line) for line in result.stdout.splitlines() if line.strip().startswith("{")])
    session_id = next(record["session_id"] for record in records if "session_id" in record)
    session = json.loads((state / "sessions" / f"{session_id}.json").read_text())
    durations = {}
    result_ids = []
    persisted = {}
    for message in session["messages"]:
        for block in message.get("content", []):
            if block.get("type") == "tool_result":
                result_ids.append(block["tool_use_id"])
                durations[block["tool_use_id"]] = message.get("tool_duration_ms")
                persisted[block["tool_use_id"]] = block
    assert result_ids == expected, f"{name}: missing/duplicate persisted tool results: {result_ids}"
    for index in error_indices:
        assert "HTTP error: 503" in json.dumps(persisted[f"call_{index}"]), f"{name}: error not persisted"
    assert all(isinstance(value, (int, float)) and value >= 0 for value in durations.values()), (
        f"{name}: missing execution timing: {durations}")
    events = snapshot["http_events"]
    tokens = [args["url"].split("/")[-3] for tool, args in calls if tool == "webfetch"]
    for event in ("start", "end"):
        observed = [entry["token"] for entry in events if entry["event"] == event]
        assert sorted(observed) == sorted(tokens), f"{name}: HTTP {event} missing/duplicated: {observed}"
    peak = snapshot["max_active_http"]
    assert snapshot["active_http"] == 0, f"{name}: unfinished HTTP requests"
    assert (2 <= peak <= 10) if enabled else peak == 1, f"{name}: unexpected HTTP concurrency {peak}"
    elapsed = snapshot["request_times"][1] - snapshot["tool_response_sent"]
    summary = {"name": name, "mode": mode, "config_parallel": config, "env_override": override,
               "enabled": enabled, "tool_round_seconds": elapsed, "durations_ms": durations,
               "max_active_http": peak, "tool_results": len(replay), "provider_requests": len(requests)}
    (case / "summary.json").write_text(json.dumps(summary, indent=2))
    print(f"PASS {name}: tool round={elapsed:.3f}s peak_http={peak} durations_ms={durations}", flush=True)
    return summary, replay, events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    assert binary.is_file(), f"binary not found: {binary}"
    if args.output_dir:
        root = args.output_dir.resolve()
        root.mkdir(parents=True, exist_ok=True)
    else:
        root = Path(tempfile.mkdtemp(prefix="parallel-cli-", dir=os.environ.get("JCODE_SCRATCH_DIR")))
    print(f"Artifacts: {root}", flush=True)
    summaries = []
    for mode in ("json", "ndjson"):
        policies = [
            ("default-off", None, None, False),
            ("config-on", True, None, True),
            ("env-on-overrides-false", False, 1, True),
            ("env-off-overrides-true", True, 0, False),
        ]
        timings = {}
        for label, config, override, enabled in policies:
            summary, _, events = run_case(binary, root, f"{mode}-{label}", mode,
                                          config, override, enabled, overlap_calls)
            summaries.append(summary)
            timings[label] = summary["tool_round_seconds"]
            if enabled:
                durations = summary["durations_ms"]
                assert durations["call_1"] < durations["call_0"] * 0.6, (
                    f"{mode}: fast execution duration includes ordered-delivery wait: {durations}")
                ends = [entry["token"] for entry in events if entry["event"] == "end"]
                assert ends.index("fast") < ends.index("slow0"), f"{mode}: fast HTTP did not finish first"
        assert timings["default-off"] > timings["config-on"] + 0.8, (
            f"{mode}: no measurable latency benefit: {timings}")
        summary, replay, events = run_case(binary, root, f"{mode}-unsafe-barriers", mode,
                                           True, None, True, barrier_calls)
        summaries.append(summary)
        starts = [entry for entry in events if entry["event"] == "start"]
        assert all(entry["barrier_exists"] == entry["token"].startswith("after") for entry in events), (
            f"{mode}: Bash barrier crossed: {events}")
        before_end = max(entry["time"] for entry in events
                         if entry["event"] == "end" and entry["token"].startswith("before"))
        after_start = min(entry["time"] for entry in starts if entry["token"].startswith("after"))
        assert before_end <= after_start, f"{mode}: HTTP crossed unsafe Bash barrier"
        assert "barrier-ok" in str(replay[6]["output"]), f"{mode}: read crossed write barrier"
        sorted_output = str(replay[8]["output"])
        assert ("1\\ta" in sorted_output or "1\ta" in sorted_output), f"{mode}: sort barrier failed"
        assert ("2\\tz" in sorted_output or "2\tz" in sorted_output), f"{mode}: sorted output incomplete"
        assert all(token in str(replay[9]["output"]) for token in ("input", "sentinel", "sorted")), (
            f"{mode}: native ls did not return useful results")
        summary, _, _ = run_case(binary, root, f"{mode}-bounded-12", mode, True, None, True,
                                  lambda base: [fetch(base, f"bounded{i}", 0.3) for i in range(12)])
        summaries.append(summary)
        summary, _, _ = run_case(binary, root, f"{mode}-http-error", mode, True, None, True,
                                  lambda base: [fetch(base, "ok0", 0.25), fetch(base, "error", 0.08, 503),
                                                fetch(base, "ok2", 0.25)], error_indices=(1,))
        summaries.append(summary)
    (root / "summary.json").write_text(json.dumps(summaries, indent=2))
    print(f"PASS all {len(summaries)} real-binary loopback cases: JSON/NDJSON opt-in policy, "
          "HTTP overlap and timing, unsafe barriers, native read/ls, cap <=10, "
          "HTTP error exactly once, two provider requests with ordered unique replay", flush=True)


if __name__ == "__main__":
    main()
