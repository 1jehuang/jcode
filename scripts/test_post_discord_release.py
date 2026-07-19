#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import threading
import unittest
import urllib.error


SCRIPT = Path(__file__).with_name("post_discord_release.py")
SPEC = importlib.util.spec_from_file_location("post_discord_release", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class WebhookHandler(BaseHTTPRequestHandler):
    request_path = ""
    payload: dict[str, object] = {}
    response_status = 200

    def do_POST(self) -> None:  # noqa: N802 - stdlib callback name
        type(self).request_path = self.path
        length = int(self.headers["Content-Length"])
        type(self).payload = json.loads(self.rfile.read(length))
        response = json.dumps(
            {"id": "discord-message-123"}
            if self.response_status == 200
            else {"message": "rejected"}
        ).encode()
        self.send_response(self.response_status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, _format: str, *_args: object) -> None:
        pass


class DiscordReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        WebhookHandler.response_status = 200

    def test_formats_release_and_removes_platform_availability(self) -> None:
        body = """## Highlights

Fast startup.

<!-- jcode-platform-availability:start -->
## Platform availability
- Linux: available
<!-- jcode-platform-availability:end -->
"""
        message = MODULE.format_message(
            tag="v0.52.0",
            name="v0.52.0",
            body=body,
            url="https://example.test/releases/v0.52.0",
        )
        self.assertEqual(message, "## v0.52.0\n## Highlights\n\nFast startup.")

    def test_truncates_to_discord_limit_and_keeps_release_link(self) -> None:
        url = "https://example.test/releases/v0.52.0"
        message = MODULE.format_message(
            tag="v0.52.0", name="A large release", body="x" * 3_000, url=url
        )
        self.assertLessEqual(len(message), MODULE.DISCORD_LIMIT)
        self.assertTrue(message.endswith(f"… (full notes: <{url}>)"))

    def test_marker_is_tag_specific(self) -> None:
        body = "notes\n\n<!-- jcode-discord-announced:v0.52.0 -->\n"
        self.assertTrue(MODULE.already_announced(body, "v0.52.0"))
        self.assertFalse(MODULE.already_announced(body, "v0.53.0"))

    def test_posts_with_wait_and_disables_mentions(self) -> None:
        server = ThreadingHTTPServer(("127.0.0.1", 0), WebhookHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            result = MODULE.post_to_discord(
                webhook_url=f"http://127.0.0.1:{server.server_port}/webhook?thread_id=1",
                content="Release @everyone",
            )
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

        self.assertEqual(result["id"], "discord-message-123")
        self.assertIn("thread_id=1", WebhookHandler.request_path)
        self.assertIn("wait=true", WebhookHandler.request_path)
        self.assertEqual(
            WebhookHandler.payload,
            {"content": "Release @everyone", "allowed_mentions": {"parse": []}},
        )

    def test_rejected_webhook_raises_http_error(self) -> None:
        WebhookHandler.response_status = 400
        server = ThreadingHTTPServer(("127.0.0.1", 0), WebhookHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with self.assertRaisesRegex(
                urllib.error.HTTPError, "HTTP Error 400"
            ) as raised:
                MODULE.post_to_discord(
                    webhook_url=f"http://127.0.0.1:{server.server_port}/webhook",
                    content="Release",
                )
            raised.exception.close()
        finally:
            server.shutdown()
            thread.join()
            server.server_close()


if __name__ == "__main__":
    unittest.main()
