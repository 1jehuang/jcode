#!/usr/bin/env python3
"""Deterministic mock jcode gateway for end-to-end iOS app testing.

Speaks the exact wire protocol from `crates/jcode-base/src/gateway.rs` on a
single TCP port, peeking the request line to route like the real gateway:
  - GET  /health   -> {status, version, gateway}
  - POST /pair     -> {token, server_name, server_version}
  - GET  /ws       -> WebSocket upgrade; newline-delimited JSON event protocol

It does NOT call an LLM. A `message` request triggers a scripted, deterministic
stream (reasoning, text deltas, a tool-call lifecycle, tokens, done) so the app
can be exercised and visually validated without network or provider cost. This
is the iOS equivalent of the removed Rust simulator: one source of honest,
repeatable behavior to develop the client against.

Self-contained: no third-party deps. Minimal hand-rolled WebSocket framing.

Run:  python3 mock_gateway.py [--port 7643] [--code 123456]
"""

import argparse
import asyncio
import base64
import hashlib
import json
import struct
import sys

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
SERVER_VERSION = "mock-0.32.0"
SERVER_NAME = "mock-jcode"
DEFAULT_MODELS = [
    "claude-api:claude-fable-5",
    "claude-api:claude-sonnet-4",
    "openai:gpt-5",
    "gemini:gemini-2.5-pro",
]


class GatewayState:
    def __init__(self, code, token):
        self.code = code
        self.token = token
        self.session_id = "mock-session-0001"
        self.title = "Mock session"
        self.model = DEFAULT_MODELS[0]
        self.messages = []
        self.token_input = 0
        self.token_output = 0
        self.reasoning_effort = "high"
        self.push_demo = False


def scenario_messages(name):
    """Pre-seeded transcripts for the layout matrix. Each is a deterministic
    content state so UI efficiency can be measured across the real range."""
    # Every real jcode tool schema carries a required `intent` (see
    # jcode-tool-core::ensure_intent_in_schema), so the fixture includes it.
    bash_tool = {
        "id": "t1", "name": "bash",
        "input": '{"command": "echo hello", "intent": "Check the shell responds"}',
        "output": "hello\n", "error": None,
    }
    if name == "empty":
        return []
    if name == "short":
        return [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "Hello! How can I help?"},
        ]
    if name == "tool":
        return [
            {"role": "user", "content": "run echo hello"},
            {"role": "assistant", "content": "Done. Output above.", "tool_data": bash_tool},
        ]
    if name == "long":
        turns = []
        for i in range(6):
            turns.append({"role": "user", "content": f"Question number {i + 1} about the codebase?"})
            turns.append({
                "role": "assistant",
                "content": (
                    f"Answer {i + 1}: here is a reasonably detailed paragraph that "
                    "wraps across multiple lines to simulate a real assistant reply "
                    "with enough text to fill vertical space and exercise scrolling."
                ),
                "tool_data": bash_tool if i % 2 == 0 else None,
            })
        return turns
    if name == "code":
        return [
            {"role": "user", "content": "show me a python snippet"},
            {"role": "assistant", "content": (
                "Sure:\n\n```python\ndef fib(n):\n    a, b = 0, 1\n    for _ in range(n):\n"
                "        a, b = b, a + b\n    return a\n```\n\nThat is iterative and O(n)."
            )},
        ]
    return []


# ---------------------------------------------------------------------------
# WebSocket framing (server side)
# ---------------------------------------------------------------------------

def ws_accept_key(key: str) -> str:
    digest = hashlib.sha1((key + WS_GUID).encode()).digest()
    return base64.b64encode(digest).decode()


def encode_text_frame(text: str) -> bytes:
    payload = text.encode("utf-8")
    header = bytearray([0x81])  # FIN + text opcode
    length = len(payload)
    if length < 126:
        header.append(length)
    elif length < 65536:
        header.append(126)
        header += struct.pack(">H", length)
    else:
        header.append(127)
        header += struct.pack(">Q", length)
    return bytes(header) + payload


def encode_control_frame(opcode: int, payload: bytes = b"") -> bytes:
    header = bytearray([0x80 | opcode, len(payload)])
    return bytes(header) + payload


async def read_frame(reader: asyncio.StreamReader):
    """Returns (opcode, payload_bytes) or None on EOF/close."""
    try:
        b = await reader.readexactly(2)
    except asyncio.IncompleteReadError:
        return None
    opcode = b[0] & 0x0F
    masked = (b[1] & 0x80) != 0
    length = b[1] & 0x7F
    if length == 126:
        ext = await reader.readexactly(2)
        length = struct.unpack(">H", ext)[0]
    elif length == 127:
        ext = await reader.readexactly(8)
        length = struct.unpack(">Q", ext)[0]
    mask = await reader.readexactly(4) if masked else b"\x00\x00\x00\x00"
    data = await reader.readexactly(length) if length else b""
    if masked:
        data = bytes(data[i] ^ mask[i % 4] for i in range(len(data)))
    return opcode, data


class WSConn:
    """Minimal server-side WebSocket connection wrapper."""

    def __init__(self, writer: asyncio.StreamWriter):
        self.writer = writer
        self._lock = asyncio.Lock()

    async def send(self, text: str):
        async with self._lock:
            self.writer.write(encode_text_frame(text))
            await self.writer.drain()

    async def pong(self, payload: bytes):
        async with self._lock:
            self.writer.write(encode_control_frame(0xA, payload))
            await self.writer.drain()

    async def close(self):
        try:
            async with self._lock:
                self.writer.write(encode_control_frame(0x8))
                await self.writer.drain()
        except Exception:
            pass


# ---------------------------------------------------------------------------
# Protocol behavior
# ---------------------------------------------------------------------------

def jline(obj):
    return json.dumps(obj)


def chunk_text(text, size):
    for i in range(0, len(text), size):
        yield text[i : i + size]


async def send_event(ws: WSConn, obj):
    await ws.send(jline(obj))


async def stream_response(ws, state, user_text, req_id):
    await send_event(ws, {"type": "ack", "id": req_id})

    for chunk in ["Looking at ", "the request", "..."]:
        await send_event(ws, {"type": "reasoning_delta", "text": chunk})
        await asyncio.sleep(0.05)
    await send_event(ws, {"type": "reasoning_done", "duration_secs": 0.4})

    intro = f"You said: {user_text}\n\nRunning a quick tool to demonstrate.\n\n"
    for ch in chunk_text(intro, 6):
        await send_event(ws, {"type": "text_delta", "text": ch})
        await asyncio.sleep(0.02)

    tool_id = f"tool-{req_id}"
    await send_event(ws, {"type": "tool_start", "id": tool_id, "name": "bash"})
    for piece in [
        '{"command":', ' "echo ', 'hello", ', '"intent": "Check ',
        'the shell responds"}',
    ]:
        await send_event(ws, {"type": "tool_input", "delta": piece})
        await asyncio.sleep(0.03)
    await send_event(ws, {"type": "tool_exec", "id": tool_id, "name": "bash"})
    await asyncio.sleep(0.2)
    await send_event(ws, {
        "type": "tool_done", "id": tool_id, "name": "bash",
        "output": "hello\n", "error": None,
    })

    answer = (
        "Done. Here is a code block:\n\n"
        "```python\nprint('hello from mock gateway')\n```\n\n"
        "And a **bold** word plus `inline code`."
    )
    for ch in chunk_text(answer, 8):
        await send_event(ws, {"type": "text_delta", "text": ch})
        await asyncio.sleep(0.02)

    await send_event(ws, {"type": "message_end"})

    state.token_input += 120 + len(user_text)
    state.token_output += 240
    await send_event(ws, {"type": "tokens", "input": state.token_input, "output": state.token_output})

    state.messages.append({"role": "user", "content": user_text})
    state.messages.append({
        "role": "assistant", "content": answer,
        "tool_data": {
            "id": tool_id, "name": "bash",
            "input": '{"command": "echo hello", "intent": "Check the shell responds"}',
            "output": "hello\n", "error": None,
        },
    })

    await send_event(ws, {"type": "done", "id": req_id})


def history_payload(state, req_id):
    return {
        "type": "history",
        "id": req_id,
        "session_id": state.session_id,
        "messages": state.messages,
        "provider_name": "anthropic-api",
        "provider_model": state.model,
        "available_models": DEFAULT_MODELS,
        "total_tokens": [state.token_input, state.token_output],
        "all_sessions": [state.session_id, "mock-session-0002"],
        "server_version": SERVER_VERSION,
        "display_title": state.title,
        "reasoning_effort": state.reasoning_effort,
    }


async def handle_request(ws, state, raw):
    try:
        msg = json.loads(raw)
    except json.JSONDecodeError:
        return
    req_type = msg.get("type")
    req_id = int(msg.get("id", 0))
    print(f"[ws] <- {req_type} id={req_id}", file=sys.stderr)

    if req_type == "subscribe":
        await send_event(ws, {"type": "ack", "id": req_id})
        await send_event(ws, {"type": "session", "session_id": state.session_id})
        await send_event(ws, {
            "type": "state", "id": req_id, "session_id": state.session_id,
            "message_count": len(state.messages), "is_processing": False,
        })
    elif req_type == "get_history":
        await send_event(ws, history_payload(state, req_id))
    elif req_type == "message":
        await stream_response(ws, state, msg.get("content", ""), req_id)
    elif req_type == "soft_interrupt":
        await send_event(ws, {"type": "ack", "id": req_id})
        # Mirror the real server: confirm the queued message was injected
        # before streaming the (echoed) response it participates in.
        await send_event(ws, {
            "type": "soft_interrupt_injected",
            "content": msg.get("content", ""),
            "display_role": "user",
            "point": "immediate",
            "tools_skipped": 0,
        })
        await stream_response(ws, state, msg.get("content", ""), req_id)
    elif req_type == "cancel":
        await send_event(ws, {"type": "interrupted"})
        await send_event(ws, {"type": "done", "id": req_id})
    elif req_type == "ping":
        await send_event(ws, {"type": "pong", "id": req_id})
    elif req_type == "set_model":
        state.model = msg.get("model", state.model)
        await send_event(ws, {"type": "model_changed", "id": req_id, "model": state.model, "error": None})
        await send_event(ws, {"type": "available_models_updated", "available_models": DEFAULT_MODELS, "provider_model": state.model})
    elif req_type == "set_reasoning_effort":
        state.reasoning_effort = msg.get("effort", state.reasoning_effort)
        await send_event(ws, {"type": "reasoning_effort_changed", "id": req_id, "effort": state.reasoning_effort, "error": None})
    elif req_type == "compact":
        await send_event(ws, {"type": "compact_result", "id": req_id, "message": "Compacted context (2048 tokens saved)", "success": True})
    elif req_type == "rename_session":
        state.title = msg.get("title") or "Untitled"
        await send_event(ws, {"type": "session_renamed", "session_id": state.session_id, "display_title": state.title})
    elif req_type == "resume_session":
        sid = msg.get("session_id", state.session_id)
        state.session_id = sid
        state.messages = []
        await send_event(ws, {"type": "session", "session_id": sid})
        await send_event(ws, history_payload(state, req_id))
    elif req_type == "clear":
        state.messages = []
        await send_event(ws, history_payload(state, req_id))
    elif req_type == "cancel_soft_interrupts":
        await send_event(ws, {"type": "ack", "id": req_id})
    elif req_type == "_notify":
        # Test-only: synthesize a push notification + a compaction notice.
        await send_event(ws, {"type": "notification", "from_name": "swarm", "message": "build finished"})
        await send_event(ws, {"type": "compaction", "trigger": "manual", "tokens_saved": 4096})
    else:
        print(f"[ws] (ignored unknown request {req_type})", file=sys.stderr)


# ---------------------------------------------------------------------------
# HTTP + routing
# ---------------------------------------------------------------------------

def http_response(status_line, body):
    body_bytes = body.encode()
    head = (
        f"HTTP/1.1 {status_line}\r\n"
        "Content-Type: application/json\r\n"
        f"Content-Length: {len(body_bytes)}\r\n"
        "Connection: close\r\n"
        "Access-Control-Allow-Origin: *\r\n"
        "\r\n"
    )
    return head.encode() + body_bytes


async def read_http_request(reader):
    """Reads headers (and body per Content-Length). Returns (method, path, headers, body)."""
    header_data = b""
    while b"\r\n\r\n" not in header_data:
        chunk = await reader.read(4096)
        if not chunk:
            break
        header_data += chunk
        if len(header_data) > 65536:
            break
    if b"\r\n\r\n" not in header_data:
        return None
    head, _, rest = header_data.partition(b"\r\n\r\n")
    lines = head.decode("latin1").split("\r\n")
    request_line = lines[0]
    parts = request_line.split()
    method, path = (parts[0], parts[1]) if len(parts) >= 2 else ("", "")
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    body = rest
    content_length = int(headers.get("content-length", "0") or "0")
    while len(body) < content_length:
        chunk = await reader.read(content_length - len(body))
        if not chunk:
            break
        body += chunk
    return method, path, headers, body


async def handle_connection(reader, writer, state):
    parsed = await read_http_request(reader)
    if parsed is None:
        writer.close()
        return
    method, path, headers, body = parsed
    path_base = path.split("?")[0]
    print(f"[http] {method} {path_base}", file=sys.stderr)

    if headers.get("upgrade", "").lower() == "websocket" and path_base == "/ws":
        await serve_websocket(reader, writer, headers, state)
        return

    if method == "GET" and path_base == "/health":
        body_str = jline({"status": "ok", "version": SERVER_VERSION, "gateway": True})
        writer.write(http_response("200 OK", body_str))
    elif method == "OPTIONS":
        writer.write(
            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\n"
            b"Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n"
            b"Access-Control-Allow-Headers: Content-Type, Authorization\r\n"
            b"Content-Length: 0\r\nConnection: close\r\n\r\n"
        )
    elif method == "POST" and path_base == "/pair":
        try:
            payload = json.loads(body.decode() or "{}")
        except Exception:
            payload = {}
        if payload.get("code", "") == state.code:
            resp = jline({"token": state.token, "server_name": SERVER_NAME, "server_version": SERVER_VERSION})
            writer.write(http_response("200 OK", resp))
        else:
            resp = jline({"error": "Invalid or expired pairing code"})
            writer.write(http_response("401 Unauthorized", resp))
    else:
        writer.write(http_response("404 Not Found", jline({"error": "Not found"})))

    try:
        await writer.drain()
    except Exception:
        pass
    writer.close()


async def serve_websocket(reader, writer, headers, state):
    key = headers.get("sec-websocket-key")
    auth = headers.get("authorization", "")
    if not key:
        writer.close()
        return
    accept = ws_accept_key(key)
    handshake = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n"
        "\r\n"
    )
    writer.write(handshake.encode())
    await writer.drain()
    ws = WSConn(writer)
    ok = auth == f"Bearer {state.token}"
    print(f"[ws] client connected (auth_ok={ok})", file=sys.stderr)

    keepalive = asyncio.create_task(keepalive_loop(ws))
    push_demo = None
    if getattr(state, "push_demo", False):
        push_demo = asyncio.create_task(push_demo_loop(ws))
    try:
        buffered = ""
        while True:
            frame = await read_frame(reader)
            if frame is None:
                break
            opcode, data = frame
            if opcode == 0x8:  # close
                break
            if opcode == 0x9:  # ping
                await ws.pong(data)
                continue
            if opcode in (0x1, 0x2):
                buffered += data.decode("utf-8", errors="replace")
                while "\n" in buffered:
                    line, buffered = buffered.split("\n", 1)
                    line = line.strip()
                    if line:
                        await handle_request(ws, state, line)
                if buffered.strip():
                    await handle_request(ws, state, buffered.strip())
                    buffered = ""
    except (asyncio.IncompleteReadError, ConnectionResetError):
        pass
    finally:
        keepalive.cancel()
        if push_demo:
            push_demo.cancel()
        await ws.close()
        writer.close()
        print("[ws] client disconnected", file=sys.stderr)


async def push_demo_loop(ws):
    """Spontaneously push out-of-band notices to validate the toast UI."""
    try:
        await asyncio.sleep(2.5)
        await send_event(ws, {"type": "notification", "from_name": "swarm", "message": "build finished"})
        await asyncio.sleep(1.5)
        await send_event(ws, {"type": "compaction", "trigger": "manual", "tokens_saved": 4096})
    except asyncio.CancelledError:
        pass
    except Exception:
        pass


async def keepalive_loop(ws):
    try:
        while True:
            await asyncio.sleep(20)
            async with ws._lock:
                ws.writer.write(encode_control_frame(0x9))  # ping
                await ws.writer.drain()
    except asyncio.CancelledError:
        pass
    except Exception:
        pass


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=7643)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--code", default="123456")
    parser.add_argument("--token", default="mocktoken0123456789abcdef")
    parser.add_argument("--push-demo", action="store_true",
                        help="spontaneously push notification + compaction notices after connect")
    parser.add_argument("--scenario", default="",
                        help="pre-seed transcript: empty|short|tool|long|code")
    args = parser.parse_args()

    state = GatewayState(args.code, args.token)
    state.push_demo = args.push_demo
    if args.scenario:
        state.messages = scenario_messages(args.scenario)

    server = await asyncio.start_server(
        lambda r, w: handle_connection(r, w, state),
        args.host,
        args.port,
    )
    print(
        f"mock gateway: http+ws on {args.host}:{args.port} "
        f"(code={args.code}, token={args.token})",
        file=sys.stderr,
    )
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
