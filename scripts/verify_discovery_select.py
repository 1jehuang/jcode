#!/usr/bin/env python3
"""End-to-end check of the Discovery browse -> select handoff.

Serves a fake catalog locally and drives the real `discover_tools` tool through
a jcode session, so the contract can be verified without model credits or the
live endpoint:

- browse lists entries and never leaks setup instructions;
- browse names `select` as the next step;
- select returns the setup instructions that browse withheld.
- selecting a name that is not in the catalog (the agent recalling a product
  from training rather than from the listing) fails loudly and points at
  `suggest`, for both the 404 and the empty-body shapes.

Usage: python scripts/verify_discovery_select.py [path/to/jcode]
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

SETUP = "npx -y demo-cards-mcp@2.1.0 && export DEMO_CARDS_KEY"
TOOLS = [
    {
        "name": "demo-cards",
        "blurb": "single-use virtual cards for agent purchases",
        "url": "https://demo-cards.example",
        "setup": SETUP,
    },
    {
        "name": "demo-ledger",
        "blurb": "spend tracking and per-agent limits",
        "url": "https://demo-ledger.example",
        "setup": "npx -y demo-ledger-mcp@1.0.0",
    },
]

# Selecting this name returns a 200 with an empty entry instead of a 404, so
# both "not in the catalog" response shapes are exercised.
NULL_ENTRY_TOOL = "demo-null"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        query = parse_qs(urlparse(self.path).query)
        selected = query.get("tool", [None])[0]
        if selected:
            match = next((tool for tool in TOOLS if tool["name"] == selected), None)
            if match is None and selected != NULL_ENTRY_TOOL:
                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            payload = {"tool": match}
        else:
            payload = {"tools": TOOLS}
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args) -> None:  # noqa: D102
        pass


def run_tool(
    jcode: str, socket: Path, session: str, payload: dict, env: dict, expect_error: bool = False
) -> str:
    result = subprocess.run(
        [
            jcode,
            "--socket",
            str(socket),
            "debug",
            "-S",
            session,
            "tool",
            f"discover_tools {json.dumps(payload, separators=(',', ':'))}",
        ],
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    if result.returncode != 0 and not expect_error:
        raise SystemExit(f"tool call failed: {result.stderr or result.stdout}")
    try:
        return str(json.loads(result.stdout)["output"])
    except (json.JSONDecodeError, KeyError):
        if expect_error:
            return result.stdout + result.stderr
        raise SystemExit(f"unparseable tool response: {result.stdout or result.stderr}")


def main() -> int:
    jcode = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("JCODE_BIN", "jcode")
    server = HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    endpoint = f"http://127.0.0.1:{server.server_port}/discovery"

    failures: list[str] = []
    with tempfile.TemporaryDirectory(prefix="jcode-discovery-e2e-") as temp:
        root = Path(temp)
        home = root / "home"
        home.mkdir()
        (home / "config.toml").write_text(
            f'[sponsors]\nenabled = true\nendpoint = "{endpoint}"\n', encoding="utf-8"
        )
        socket = root / "jcode.sock"
        env = {
            **os.environ,
            "JCODE_HOME": str(home),
            "JCODE_RUNTIME_DIR": str(root),
            "JCODE_DISCOVERY_BENCHMARK": "1",
        }
        workspace = root / "workspace"
        workspace.mkdir()

        server_process = subprocess.Popen(
            [jcode, "--socket", str(socket), "--no-selfdev", "--no-update", "serve",
             "--server-name", f"discovery-e2e-{os.getpid()}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=env,
            start_new_session=True,
        )
        try:
            deadline = threading.Event()
            for _ in range(100):
                probe = subprocess.run(
                    [jcode, "--socket", str(socket), "debug", "server:info"],
                    capture_output=True, text=True, env=env,
                )
                if probe.returncode == 0:
                    break
                deadline.wait(0.2)
            else:
                raise SystemExit("benchmark server did not start")

            created = json.loads(
                subprocess.run(
                    [jcode, "--socket", str(socket), "debug", f"create_session:{workspace}"],
                    capture_output=True, text=True, env=env, timeout=60,
                ).stdout
            )
            session = created["session_id"]

            browse = run_tool(
                jcode, socket, session,
                {
                    "category": "payments",
                    "query": "virtual card capability for agent initiated online purchases",
                    "reason": "The task needs a spending-limited payment method and no current tool provides one.",
                },
                env,
            )
            print("--- browse ---")
            print(browse)
            if "demo-cards" not in browse:
                failures.append("browse did not list catalog entries")
            if SETUP in browse or "demo-cards-mcp" in browse:
                failures.append("browse leaked setup instructions")
            if "action `select`" not in browse:
                failures.append("browse did not direct the agent to select")

            select = run_tool(
                jcode, socket, session,
                {
                    "action": "select",
                    "category": "payments",
                    "tool": "demo-cards",
                    "query": "virtual card capability for agent initiated online purchases",
                    "reason": "Single-use cards with a hard spending limit match the requested constraint exactly.",
                },
                env,
            )
            print("--- select ---")
            print(select)
            if "demo-cards-mcp@2.1.0" not in select:
                failures.append("select did not return the withheld setup instructions")

            # An agent that skips browse (or ignores it) and selects a product
            # it remembers must be told plainly that the catalog does not carry
            # it, not handed a generic endpoint error it may treat as flaky.
            for off_catalog, shape in (("stripe", "404"), (NULL_ENTRY_TOOL, "empty entry")):
                rejected = run_tool(
                    jcode, socket, session,
                    {
                        "action": "select",
                        "category": "payments",
                        "tool": off_catalog,
                        "query": "virtual card capability for agent initiated online purchases",
                        "reason": "Reaching for a payments product recalled from training rather than the listing.",
                    },
                    env,
                    expect_error=True,
                )
                print(f"--- off-catalog select ({shape}) ---")
                print(rejected)
                if "not in the Jcode catalog" not in rejected:
                    failures.append(f"off-catalog select ({shape}) was not identified as off-catalog")
                if "action `suggest`" not in rejected:
                    failures.append(f"off-catalog select ({shape}) did not point at suggest")
                if SETUP in rejected:
                    failures.append(f"off-catalog select ({shape}) leaked setup instructions")
        finally:
            subprocess.run(
                [jcode, "--socket", str(socket), "server", "stop"],
                capture_output=True, text=True, env=env,
            )
            server_process.terminate()
            server.shutdown()

    if failures:
        print("\nFAILED:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print(
        "\nOK: browse withholds setup, names select, select delivers it, and off-catalog "
        "selects are rejected with a suggest path."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
