#!/usr/bin/env python3
"""Local-only Discovery fixture for evaluating an OpenRelay/Rivet listing.

This server is intentionally separate from the hosted Discovery catalog. It
implements the browse/select response contract used by jcode and never sends
analytics, sponsor reports, or usage data anywhere.
"""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

CATEGORIES = {
    "payments",
    "code-review",
    "databases",
    "browser-automation",
    "deployment",
    "observability",
    "authentication",
    "security",
    "storage",
    "analytics",
    "web-search",
    "web-data",
    "financial-data",
    "cloud-infrastructure",
    "compliance-and-privacy",
    "integration-platforms",
    "email-messaging",
    "ai-models",
    "other",
}

TOOL = {
    "name": "openrelay-rivet",
    "blurb": "public Ethereum Classic JSON-RPC endpoint operated by OpenRelay",
    "url": "https://rivet.link/networks?via=jcode-discovery-test",
    "setup": "\n".join(
        [
            "Complete agent setup protocol (no signup or documentation fetch is required):",
            "",
            "1. No account, package installation, API key, environment variable, payment, or MCP connection is required. Setup is complete with this Discovery response.",
            "2. OpenRelay operates Rivet's public Ethereum Classic JSON-RPC endpoint at https://etc.rivet.link/.",
            "3. Use the existing bash tool to POST JSON-RPC with Content-Type: application/json. For read-only chain inspection, use eth_chainId, eth_blockNumber, and eth_getBlockByNumber with params [\"latest\", false].",
            "4. Example: curl -fsS -X POST https://etc.rivet.link/ -H 'content-type: application/json' --data '{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"latest\",false],\"id\":1}'.",
            "5. JSON-RPC quantities are hexadecimal. Convert the block number and timestamp locally; the block timestamp is Unix seconds in UTC.",
            "6. This fixture is for read-only testing. Do not call signing, account-management, or transaction-submission methods.",
        ]
    ),
}

POLICY = "test discovery fixture: local evaluation only, not a production listing"
DETAILS = "https://openrelay.xyz"


class Handler(BaseHTTPRequestHandler):
    server_version = "OpenRelayDiscoveryTest/1"

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path not in {"/", "/v1/discovery"}:
            self._json(404, {"error": {"code": "not_found", "message": "not found"}})
            return

        params = parse_qs(parsed.query)
        category = params.get("category", [""])[0].strip().lower()
        selected = params.get("tool", [""])[0].strip().lower()
        query = params.get("q", [""])[0].strip()
        reason = params.get("reason", [""])[0].strip()

        self._log_request(category, selected, query, reason)

        if category not in CATEGORIES:
            self._json(
                400,
                {
                    "error": {
                        "code": "unknown_category",
                        "message": "unknown Discovery category",
                    }
                },
            )
            return

        if selected:
            if category != "cloud-infrastructure" or selected != TOOL["name"]:
                self._json(
                    404,
                    {
                        "error": {
                            "code": "unknown_tool",
                            "message": f"no tool '{selected}' in category '{category}'",
                        }
                    },
                )
                return
            self._json(
                200,
                {
                    "category": category,
                    "tool": TOOL,
                    "policy": POLICY,
                    "details": DETAILS,
                },
            )
            return

        tools = []
        if category == "cloud-infrastructure":
            tools.append({key: TOOL[key] for key in ("name", "blurb", "url")})
        self._json(
            200,
            {
                "category": category,
                "tools": tools,
                "policy": POLICY,
                "details": DETAILS,
            },
        )

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def _log_request(
        self, category: str, selected: str, query: str, reason: str
    ) -> None:
        event = {
            "event": "discovery_request",
            "phase": "select" if selected else "browse",
            "category": category,
            "tool": selected or None,
            "query": query,
            "reason": reason,
        }
        print(json.dumps(event, separators=(",", ":")), flush=True)

    def _json(self, status: int, payload: object) -> None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--ready-file", type=Path)
    args = parser.parse_args()

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    host, port = server.server_address[:2]
    ready = {"host": host, "port": port, "endpoint": f"http://{host}:{port}/v1/discovery"}
    if args.ready_file:
        args.ready_file.write_text(json.dumps(ready) + "\n", encoding="utf-8")
    print(json.dumps({"event": "ready", **ready}, separators=(",", ":")), flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
