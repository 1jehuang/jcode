#!/usr/bin/env python3
"""Fake `claude` for jcode's Claude Code runtime tests.

Speaks the Agent SDK stream-json protocol the same way the real CLI does
(shapes copied from captures of claude 2.1.289). Behaviour is chosen by the
first word of each user prompt:

  text        -> stream "Hello from fake claude" and finish
  tool        -> Claude Code runs its own Read tool (assistant tool_use + user tool_result)
  perm        -> ask can_use_tool for Write, report the decision as text
  mcp         -> call the jcode SDK MCP tool `echo` and report its result
  ratelimit   -> rejected rate_limit_event, then park until interrupted
  authfail    -> authentication_failed assistant error + 401 result
  crash       -> exit(3) before answering (only once per FAKE_CLAUDE_STATE dir)
  slow        -> park until interrupted
  session     -> reply with the argv session flags it was started with

Every invocation's argv is appended to $FAKE_CLAUDE_LOG.
"""

import json
import os
import sys
import uuid

argv = sys.argv[1:]
log_path = os.environ.get("FAKE_CLAUDE_LOG")
if log_path:
    with open(log_path, "a") as f:
        f.write(json.dumps({"argv": argv, "config_dir": os.environ.get("CLAUDE_CONFIG_DIR"), "cwd": os.getcwd()}) + "\n")

if "-p" in argv:
    # One-shot mode (`complete_simple`): prompt on stdin, JSON result on stdout.
    prompt = sys.stdin.read().strip()
    print(json.dumps({"type": "result", "subtype": "success", "is_error": False,
                      "result": "oneshot:" + prompt}))
    sys.exit(0)

session_id = None
resume = None
model = "claude-opus-5-5"
for i, arg in enumerate(argv):
    if arg.startswith("--session-id="):
        session_id = arg.split("=", 1)[1]
    elif arg.startswith("--resume="):
        resume = arg.split("=", 1)[1]
    elif arg == "--model" and i + 1 < len(argv):
        model = argv[i + 1]

state_dir = os.environ.get("FAKE_CLAUDE_STATE", "/tmp")
known_path = os.path.join(state_dir, "sessions.json")


def load_known():
    try:
        with open(known_path) as f:
            return set(json.load(f))
    except Exception:
        return set()


def save_known(ids):
    with open(known_path, "w") as f:
        json.dump(sorted(ids), f)


known = load_known()
if resume is not None:
    if resume not in known:
        print(json.dumps({"type": "result", "subtype": "error_during_execution", "is_error": True,
                          "num_turns": 0, "session_id": str(uuid.uuid4())}), flush=True)
        sys.stderr.write(f"No conversation found with session ID: {resume}\n")
        sys.exit(1)
    session_id = resume
if session_id is None:
    session_id = str(uuid.uuid4())
known.add(session_id)
save_known(known)

expose_mcp = "--mcp-config" in argv


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


pending = {}
counter = [0]


def control_request(request):
    counter[0] += 1
    rid = f"cli-{counter[0]}"
    send({"type": "control_request", "request_id": rid, "request": request})
    return rid


def read_line():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


interrupted = [False]


def wait_response(rid):
    """Read stdin until the control_response for rid arrives."""
    while True:
        msg = read_line()
        if msg.get("type") == "control_response" and msg["response"].get("request_id") == rid:
            return msg["response"]
        if msg.get("type") == "control_request":
            handle_host_control(msg)
            if interrupted[0]:
                return None


def handle_host_control(msg):
    req = msg["request"]
    sub = req.get("subtype")
    rid = msg["request_id"]
    if sub == "initialize":
        if req.get("sdkMcpServers") and expose_mcp:
            # The real CLI handshakes with the SDK server before answering.
            r = control_request({"subtype": "mcp_message", "server_name": "jcode",
                                 "message": {"method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}},
                                             "jsonrpc": "2.0", "id": 0}})
            wait_response(r)
            control_request({"subtype": "mcp_message", "server_name": "jcode",
                             "message": {"jsonrpc": "2.0", "method": "notifications/initialized"}})
            r = control_request({"subtype": "mcp_message", "server_name": "jcode",
                                 "message": {"method": "tools/list", "jsonrpc": "2.0", "id": 1}})
            resp = wait_response(r)
            tools = resp["response"]["mcp_response"]["result"]["tools"]
            global mcp_tools
            mcp_tools = [t["name"] for t in tools]
        send({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": {
            "account": {"email": "fake@example.com", "subscriptionType": "Claude Max", "organization": "Fake Org"},
            "models": [{"value": "default", "resolvedModel": "claude-opus-5-5"},
                       {"value": "sonnet", "resolvedModel": "claude-sonnet-5"}],
            "commands": [], "current_permission_mode": "default"}}})
    elif sub == "interrupt":
        interrupted[0] = True
        send({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": {}}})
    elif sub == "set_model":
        global model
        model = req.get("model", model)
        send({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": {}}})
    else:
        send({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": "unsupported"}})


mcp_tools = []


def assistant_text(text, msg_id=None):
    send({"type": "assistant", "parent_tool_use_id": None, "session_id": session_id,
          "message": {"id": msg_id or f"msg_{uuid.uuid4().hex[:12]}", "role": "assistant", "model": model,
                      "content": [{"type": "text", "text": text}],
                      "usage": {"input_tokens": 5, "output_tokens": 7}}})


def result(user_uuid, subtype="success", is_error=False, extra=None):
    body = {"type": "result", "subtype": subtype, "is_error": is_error, "num_turns": 1,
            "session_id": session_id, "stop_reason": "end_turn", "result": "",
            "usage": {"input_tokens": 5, "output_tokens": 7}}
    if user_uuid:
        body["user_message_uuids"] = [user_uuid]
    if extra:
        body.update(extra)
    send(body)


def park_until_interrupt(user_uuid):
    while not interrupted[0]:
        msg = read_line()
        if msg.get("type") == "control_request":
            handle_host_control(msg)
    result(user_uuid, subtype="error_during_execution", is_error=True,
           extra={"terminal_reason": "aborted_streaming", "errors": ["Request was aborted (interrupt)"]})
    interrupted[0] = False


def turn(msg):
    user_uuid = msg.get("uuid")
    content = msg["message"]["content"]
    text = content if isinstance(content, str) else " ".join(
        b.get("text", "") for b in content if b.get("type") == "text")
    # The prompt may carry a <previous_conversation> preamble; the command is
    # the first word after it.
    if "</previous_conversation>" in text:
        text = text.split("</previous_conversation>", 1)[1]
    words = text.strip().split()
    cmd = words[0] if words else "text"
    global last_prompt
    last_prompt = msg["message"]["content"]
    send({"type": "system", "subtype": "init", "session_id": session_id, "model": model,
          "tools": ["Read", "Write", "Bash"] + [f"mcp__jcode__{t}" for t in mcp_tools]})

    if cmd == "text":
        mid = "msg_text1"
        send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id,
              "event": {"type": "message_start", "message": {"id": mid}}})
        send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id,
              "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello from "}}})
        send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id,
              "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "fake claude"}}})
        assistant_text("Hello from fake claude", mid)
        send({"type": "stream_event", "parent_tool_use_id": None, "session_id": session_id,
              "event": {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"input_tokens": 5, "output_tokens": 7}}})
        send({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "resetsAt": 1791576000,
              "rateLimitType": "five_hour", "utilization": 0.25,
              "unifiedWindows": {"five_hour": {"utilization": 0.25, "resetsAt": 1791576000},
                                 "seven_day": {"utilization": 0.5, "resetsAt": 1791900000}}}})
        result(user_uuid)
    elif cmd == "tool":
        tid = "toolu_read1"
        send({"type": "assistant", "parent_tool_use_id": None, "session_id": session_id,
              "message": {"id": "msg_tool1", "role": "assistant", "content": [
                  {"type": "tool_use", "id": tid, "name": "Read", "input": {"file_path": "/tmp/x.txt"}}]}})
        send({"type": "user", "parent_tool_use_id": None, "session_id": session_id,
              "message": {"role": "user", "content": [{"tool_use_id": tid, "type": "tool_result", "content": "1\thello"}]}})
        assistant_text("The file says hello")
        result(user_uuid)
    elif cmd == "perm":
        rid = control_request({"subtype": "can_use_tool", "tool_name": "Write",
                               "input": {"file_path": "/tmp/p.txt", "content": "x"}, "tool_use_id": "toolu_w"})
        resp = wait_response(rid)
        behavior = resp["response"]["behavior"]
        assistant_text(f"permission:{behavior}")
        result(user_uuid)
    elif cmd == "mcp":
        name = words[1] if len(words) > 1 else "echo"
        rid = control_request({"subtype": "can_use_tool", "tool_name": f"mcp__jcode__{name}",
                               "input": {"text": "MCPOK"}, "tool_use_id": "toolu_m"})
        perm = wait_response(rid)
        if perm["response"]["behavior"] != "allow":
            assistant_text("mcp:denied")
            result(user_uuid)
            return
        send({"type": "assistant", "parent_tool_use_id": None, "session_id": session_id,
              "message": {"id": "msg_mcp1", "role": "assistant", "content": [
                  {"type": "tool_use", "id": "toolu_m", "name": f"mcp__jcode__{name}", "input": {"text": "MCPOK"}}]}})
        rid = control_request({"subtype": "mcp_message", "server_name": "jcode",
                               "message": {"method": "tools/call", "params": {"name": name, "arguments": {"text": "MCPOK"}},
                                           "jsonrpc": "2.0", "id": 2}})
        resp = wait_response(rid)
        if resp is None:
            park_until_interrupt(user_uuid)
            return
        res = resp["response"]["mcp_response"]["result"]
        out = res["content"][0]["text"]
        send({"type": "user", "parent_tool_use_id": None, "session_id": session_id,
              "message": {"role": "user", "content": [{"tool_use_id": "toolu_m", "type": "tool_result",
                                                        "content": [{"type": "text", "text": out}], "is_error": res.get("isError", False)}]}})
        assistant_text(f"mcp:{out}")
        result(user_uuid)
    elif cmd == "ratelimit":
        send({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": 4102444800,
              "rateLimitType": "five_hour", "utilization": 1.0}})
        park_until_interrupt(user_uuid)
    elif cmd == "authfail":
        send({"type": "assistant", "parent_tool_use_id": None, "session_id": session_id, "error": "authentication_failed",
              "message": {"id": "msg_auth", "role": "assistant", "content": [{"type": "text", "text": "Invalid API key"}]}})
        result(user_uuid, subtype="success", is_error=True, extra={"api_error_status": 401, "result": "Invalid API key"})
    elif cmd == "crash":
        marker = os.path.join(state_dir, "crashed")
        if not os.path.exists(marker):
            open(marker, "w").close()
            sys.stderr.write("fake claude crashed\n")
            sys.stderr.flush()
            os._exit(3)
        assistant_text("recovered after crash")
        result(user_uuid)
    elif cmd == "slow":
        park_until_interrupt(user_uuid)
    elif cmd == "session":
        assistant_text("session:" + ("resume" if resume else "new") + ":" + session_id)
        result(user_uuid)
    elif cmd == "echo-prompt":
        # Reply with the full text the host sent (checks transcript seeding).
        assistant_text(json.dumps(last_prompt))
        result(user_uuid)
    elif cmd == "model":
        assistant_text("model:" + model)
        result(user_uuid)
    else:
        assistant_text("unknown command " + cmd)
        result(user_uuid)


last_prompt = None

while True:
    msg = read_line()
    kind = msg.get("type")
    if kind == "control_request":
        handle_host_control(msg)
    elif kind == "user":
        turn(msg)
