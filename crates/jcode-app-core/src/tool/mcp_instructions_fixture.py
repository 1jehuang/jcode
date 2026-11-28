"""Offline MCP initialization peer for operational-guidance tests."""
import json
import sys

mode = sys.argv[1]
guidance = {"normal": "Use the documented workspace before requesting data.",
            "blank": " \n\t ", "huge": "雪" * 40000, "boundary": "雪" * 16384}[mode]
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    if request["method"] == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {},
                  "serverInfo": {"name": "guidance-fixture", "version": "1"},
                  "instructions": guidance}
    elif request["method"] == "tools/list":
        result = {"tools": [{"name": name, "description": name,
                             "inputSchema": {"type": "object"}} for name in ["lookup", "read"]]}
    else:
        raise AssertionError(request["method"])
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}, ensure_ascii=False) + "\n")
    sys.stdout.flush()
