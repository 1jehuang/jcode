"""Tiny deterministic LSP peer, used only by Rust transport tests."""
import json
import os
import sys


def send(value):
    data = json.dumps(value, ensure_ascii=False).encode("utf-8")
    header = f"Content-Length: {len(data)}\r\n\r\n".encode("ascii")
    # Separate writes exercise buffered frame parsing.
    sys.stdout.buffer.write(header)
    sys.stdout.buffer.flush()
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()


initialized = False
opened = False
while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        key, value = line.decode("ascii").split(":", 1)
        headers[key.lower()] = value.strip()
    message = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    method = message.get("method")
    if method == "exit":
        sys.exit(0)
    if method == "initialized":
        initialized = True
    if method == "textDocument/didOpen":
        assert initialized
        opened = True
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": message["params"]["textDocument"]["uri"], "diagnostics": [{"message": "fixture diagnostic"}]}})
    if "id" not in message:
        continue
    if method == "initialize":
        result = {"capabilities": {"hoverProvider": True}}
        if os.environ.get("FIXTURE_DATA_REPORT"):
            data = os.environ["CLAUDE_PLUGIN_DATA"]
            marker = os.path.join(data, "lsp-fixture-marker")
            existed = os.path.isfile(marker)
            with open(marker, "w", encoding="utf-8") as output:
                output.write("persistent")
            result["fixtureData"] = {
                "path": data, "exists": os.path.isdir(data),
                "previousMarker": existed, "arg": sys.argv[1],
                "params": message.get("params"),
            }
        if os.environ.get("FIXTURE_PULL_DIAGNOSTICS"):
            result["capabilities"]["diagnosticProvider"] = {}
    elif method == "shutdown":
        result = None
    elif method == "fixture/error":
        send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32603, "message": "fixture error"}})
        continue
    else:
        assert initialized and opened
        result = {"method": method, "params": message["params"], "text": "café λ"}
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})
