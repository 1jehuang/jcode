"""Offline line-delimited MCP peer for catalog pagination tests."""
import json
import sys
import time

mode = sys.argv[1]
round_number = 0
page_number = 0
opaque = " next +/雪 "


def tool(name, description="fixture"):
    return {"name": name, "description": description, "inputSchema": {"type": "object"}}


for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    response = {"jsonrpc": "2.0", "id": message["id"]}
    method = message["method"]
    if method == "initialize":
        response["result"] = {"protocolVersion": "2024-11-05", "capabilities": {},
                              "serverInfo": {"name": "pagination-fixture", "version": "1"}}
    elif method == "tools/list":
        cursor = (message.get("params") or {}).get("cursor")
        if cursor is None:
            round_number += 1
            page_number = 0
        page_number += 1
        if mode == "normal":
            if cursor is None:
                response["result"] = {"tools": [tool("first")], "nextCursor": opaque}
            elif cursor == opaque:
                response["result"] = {"tools": [tool("second")]}
            else:
                response["error"] = {"code": -32602, "message": "cursor was changed"}
        elif round_number == 1:
            response["result"] = {"tools": [tool("seed")]}
        elif mode == "empty":
            response["result"] = {"tools": []}
        elif mode == "oversize":
            response["result"] = {"tools": [tool("huge", "x" * (32 * 1024 * 1024))]}
        elif mode == "count":
            response["result"] = {"tools": [tool(f"tool-{index}") for index in range(10001)]}
        elif mode == "cycle":
            response["result"] = {"tools": [tool(f"partial-{page_number}")], "nextCursor": "repeat"}
        elif mode == "many":
            response["result"] = {"tools": [], "nextCursor": str(page_number)}
        elif mode == "deadline":
            time.sleep(0.15)
            response["result"] = {"tools": [tool(f"partial-{page_number}")], "nextCursor": str(page_number)}
        elif page_number == 1:
            response["result"] = {"tools": [tool("partial")], "nextCursor": opaque}
        elif mode == "duplicate":
            response["result"] = {"tools": [tool("partial")]}
        elif mode == "error":
            response["error"] = {"code": -32603, "message": "later page failed"}
        elif mode == "invalid":
            response["result"] = {"tools": "invalid"}
        elif mode != "missing":
            raise AssertionError(mode)
    else:
        response["error"] = {"code": -32601, "message": "unsupported fixture method"}
    sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
    sys.stdout.flush()
