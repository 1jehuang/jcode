#!/usr/bin/env python3
"""End-to-end check of the Discovery browse -> select handoff.

Serves a fake catalog locally and drives the real `discover_tools` tool through
a jcode session, so the contract can be verified without model credits or the
live endpoint:

- browse lists entries and never leaks setup instructions;
- browse names `select` as the next step;
- catalog select returns the setup instructions that browse withheld;
- off-catalog select records the exact chosen product without returning provider
  information or setup instructions.

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

OFF_CATALOG_TOOL = "demo-other"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        query = parse_qs(urlparse(self.path).query)
        selected = query.get("tool", [None])[0]
        if selected:
            match = next((tool for tool in TOOLS if tool["name"] == selected), None)
            payload = {
                "category": "payments",
                "selected_tool": selected,
                "listed": match is not None,
            }
            if match is not None:
                payload["tool"] = match
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
                    "action": "search",
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

            off_catalog = run_tool(
                jcode, socket, session,
                {
                    "action": "select",
                    "category": "payments",
                    "tool": OFF_CATALOG_TOOL,
                    "query": "virtual card capability for agent initiated online purchases",
                    "reason": "The user explicitly chose this other product after comparing the available options.",
                },
                env,
            )
            print("--- off-catalog select ---")
            print(off_catalog)
            if f"Selected off-catalog product '{OFF_CATALOG_TOOL}'" not in off_catalog:
                failures.append("off-catalog select did not record the exact product")
            if "Selection recorded as demand data" not in off_catalog:
                failures.append("off-catalog select did not return a demand-data receipt")
            if "no provider information" not in off_catalog:
                failures.append("off-catalog select did not disclose the absence of provider data")
            if "Setup:" in off_catalog or SETUP in off_catalog or "https://" in off_catalog:
                failures.append("off-catalog select leaked provider or setup information")
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
        "\nOK: browse withholds setup and names select; catalog select returns setup; "
        "off-catalog select records demand without provider or setup information."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
