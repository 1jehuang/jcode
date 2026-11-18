#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import io
import tempfile
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import threading
import unittest
import urllib.error
from unittest import mock


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

    def test_checked_in_announcement_validation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "v-test.json"
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", Path(directory)):
                self.assertIsNone(MODULE.load_announcement("missing"))
                self.assertIsNone(MODULE.load_announcement("../v-test"))
                for value in ([], {}, [""], [4], ["😀" * 1001], ["a", "x" * 2001]):
                    path.write_text(json.dumps(value))
                    with self.subTest(value_type=type(value)), self.assertRaises(RuntimeError):
                        MODULE.load_announcement("v-test")
                path.write_text(json.dumps(["😀" * 1000, "second"]))
                self.assertEqual(MODULE.load_announcement("v-test"), ["😀" * 1000, "second"])
                path.write_text("not json")
                with self.assertRaises(RuntimeError):
                    MODULE.load_announcement("v-test")

    def multipart_http(self, *, fail_receipt=False, fail_delivery=False, fail_pending=False):
        release = {"id": 100, "body": "notes", "html_url": "https://example.test/v-test"}
        posts = []
        patches = []

        def request(req, timeout):
            if "discord.test" in req.full_url:
                posts.append(json.loads(req.data))
                if fail_delivery:
                    raise urllib.error.URLError("response lost")
                # Simulate a concurrent human edit while Discord is in flight.
                release["body"] += "\nhuman edit"
                result = {"id": str(len(posts))}
            elif req.method == "PATCH":
                body = json.loads(req.data)["body"]
                if fail_pending and "pending" in body:
                    raise urllib.error.URLError("pending write failed")
                if fail_receipt and "sent:" in body:
                    raise urllib.error.URLError("receipt write failed")
                patches.append(body)
                release["body"] = body
                result = dict(release)
            else:
                result = dict(release)
            return io.BytesIO(json.dumps(result).encode())

        return release, posts, patches, request

    def run_multipart(self):
        return MODULE.announce_release(repository="owner/repo", tag="v-test", token="fake",
                                       webhook_url="https://discord.test/webhook")

    def test_multipart_http_receipts_and_resume(self) -> None:
        release, posts, patches, request = self.multipart_http()
        with (mock.patch.object(MODULE, "load_announcement", return_value=["first", "second"]),
              mock.patch.object(MODULE.urllib.request, "urlopen", side_effect=request)):
            self.assertEqual(self.run_multipart(), "2")
            self.assertEqual([p["content"] for p in posts], ["first", "second"])
            self.assertTrue(all(p["allowed_mentions"] == {"parse": []} for p in posts))
            self.assertEqual(sum("human edit" in p for p in patches), 4)
            self.assertIn("sent:1", release["body"])
            self.assertIn("sent:2", release["body"])
            self.assertIn(MODULE.announcement_marker("v-test"), release["body"])
            self.assertIsNone(self.run_multipart())
            self.assertEqual(len(posts), 2)
            # A lost final marker is safe: all individual receipts remain.
            release["body"] = release["body"].replace(MODULE.announcement_marker("v-test"), "")
            self.run_multipart()
            self.assertEqual(len(posts), 2)
            # Resume from the first durable receipt, not the beginning.
            release["body"] = patches[1]
            self.run_multipart()
            self.assertEqual([p["content"] for p in posts], ["first", "second", "second"])

    def test_pending_checkpoint_blocks_duplicates_after_uncertain_delivery(self) -> None:
        for failure in ("receipt", "delivery"):
            release, posts, _, request = self.multipart_http(
                fail_receipt=failure == "receipt", fail_delivery=failure == "delivery")
            with (self.subTest(failure=failure),
                  mock.patch.object(MODULE, "load_announcement", return_value=["first", "second"]),
                  mock.patch.object(MODULE.urllib.request, "urlopen", side_effect=request),
                  mock.patch("sys.stderr", new_callable=io.StringIO) as stderr):
                self.run_multipart()
                self.assertIn("Partial Discord announcement", stderr.getvalue())
                self.assertIn("pending", release["body"])
                with self.assertRaisesRegex(RuntimeError, "Reconcile manually"):
                    self.run_multipart()
                self.assertEqual(len(posts), 1)
                self.assertNotIn(MODULE.announcement_marker("v-test"), release["body"])

    def test_pending_write_failure_never_sends(self) -> None:
        _, posts, _, request = self.multipart_http(fail_pending=True)
        with (mock.patch.object(MODULE, "load_announcement", return_value=["first"]),
              mock.patch.object(MODULE.urllib.request, "urlopen", side_effect=request),
              self.assertRaises(urllib.error.URLError)):
            self.run_multipart()
        self.assertEqual(posts, [])

    def test_changed_manifest_refuses_resume(self) -> None:
        release, posts, patches, request = self.multipart_http()
        with mock.patch.object(MODULE.urllib.request, "urlopen", side_effect=request):
            with mock.patch.object(MODULE, "load_announcement", return_value=["first", "second"]):
                self.run_multipart()
            release["body"] = patches[1]
            with (mock.patch.object(MODULE, "load_announcement", return_value=["changed", "second"]),
                  self.assertRaisesRegex(RuntimeError, "content changed")):
                self.run_multipart()
        self.assertEqual(len(posts), 2)

    def test_preview_is_commit_list_with_single_link(self) -> None:
        body = (
            "## What changed\n\n5 commits since **v0.9**.\n\n"
            "[Full comparison](https://example.test/compare/a...b)\n\n"
            "**Highlights from commit titles**\n\n- Highlight only\n\n"
            "## Commits\n\n"
            "- [abc12345](https://example.test/commit/abc12345) Fix picker\n"
            "- [def67890](https://example.test/commit/def67890) feat: see https://x.test/y\n"
            "\n## First-time preview setup\nDO NOT INCLUDE\n"
        )
        message = MODULE.format_message(
            tag="v1.0.0-preview.12", name="Preview", body=body,
            url="https://example.test/releases/preview.12",
        )
        self.assertEqual(message, (
            "**jcode v1.0.0-preview.12** (preview) https://example.test/releases/preview.12\n"
            "- Fix picker\n- feat: see <https://x.test/y>"))
        self.assertEqual(len(re.findall(r"(?<!<)https?://", message)), 1)

    def test_preview_without_commits_is_header_only(self) -> None:
        for body in ("", "@everyone legacy notes", "## What changed\n\n"):
            with self.subTest(body=body):
                message = MODULE.format_message(
                    tag="v1.0.0-preview.12", name="Preview", body=body,
                    url="https://example.test/preview",
                )
                self.assertEqual(message, "**jcode v1.0.0-preview.12** (preview) https://example.test/preview")

    def test_preview_long_commit_list_is_truncated(self) -> None:
        for url in ("https://example.test/preview", "https://example.test/" + "x" * 10_000):
            with self.subTest(url_length=len(url)):
                body = "## Commits\n" + "- [abcdef12](https://e.test/c) Fix picker\n" * 5_000
                message = MODULE.format_message(
                    tag="v1.0.0-preview.12", name="Preview", body=body, url=url)
                self.assertLessEqual(len(message), MODULE.DISCORD_LIMIT)
                self.assertRegex(message, r"\n… \d+ more$")
                self.assertEqual(len(re.findall(r"(?<!<)https?://", message)), 1)

    def test_preview_announcement_requires_public_prerelease(self) -> None:
        for flags in ({"draft": True, "prerelease": True},
                      {"draft": False, "prerelease": False}):
            with (self.subTest(flags=flags),
                  mock.patch.object(MODULE, "fetch_release", return_value=flags),
                  mock.patch.object(MODULE, "post_to_discord") as post,
                  self.assertRaises(RuntimeError)):
                MODULE.announce_release(repository="owner/repo", tag="v1.0.0-preview.12",
                                        token="token", webhook_url="https://discord.test/webhook")
            post.assert_not_called()

    def test_preview_reuses_tag_specific_duplicate_prevention(self) -> None:
        tag = "v1.0.0-preview.12"
        release = {"draft": False, "prerelease": True,
                   "body": MODULE.announcement_marker(tag)}
        with (mock.patch.object(MODULE, "fetch_release", return_value=release),
              mock.patch.object(MODULE, "post_to_discord") as post):
            self.assertIsNone(MODULE.announce_release(repository="owner/repo", tag=tag,
                              token="token", webhook_url="https://discord.test/webhook"))
        post.assert_not_called()

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

    def test_announce_skips_an_existing_marker(self) -> None:
        release = {
            "id": 52,
            "name": "v0.52.0",
            "body": "notes\n<!-- jcode-discord-announced:v0.52.0 -->",
            "html_url": "https://example.test/v0.52.0",
            "draft": False,
        }
        with (
            mock.patch.object(MODULE, "fetch_release", return_value=release),
            mock.patch.object(MODULE, "post_to_discord") as post,
        ):
            result = MODULE.announce_release(
                repository="owner/repo",
                tag="v0.52.0",
                token="token",
                webhook_url="https://discord.test/webhook",
            )
        self.assertIsNone(result)
        post.assert_not_called()

    def test_announce_rejects_draft_release(self) -> None:
        release = {
            "id": 53,
            "name": "v0.53.0",
            "body": "notes",
            "html_url": "https://example.test/v0.53.0",
            "draft": True,
        }
        with (
            mock.patch.object(MODULE, "fetch_release", return_value=release),
            mock.patch.object(MODULE, "post_to_discord") as post,
            self.assertRaisesRegex(RuntimeError, "draft release"),
        ):
            MODULE.announce_release(
                repository="owner/repo",
                tag="v0.53.0",
                token="token",
                webhook_url="https://discord.test/webhook",
            )
        post.assert_not_called()

    def test_announce_refetches_before_writing_marker(self) -> None:
        initial = {
            "id": 54,
            "name": "v0.54.0",
            "body": "initial notes",
            "html_url": "https://example.test/v0.54.0",
            "draft": False,
        }
        latest = {**initial, "body": "notes edited during post"}
        with (
            mock.patch.object(MODULE, "fetch_release", side_effect=[initial, latest]),
            mock.patch.object(
                MODULE, "post_to_discord", return_value={"id": "message-54"}
            ),
            mock.patch.object(MODULE, "mark_release_announced") as mark,
        ):
            result = MODULE.announce_release(
                repository="owner/repo",
                tag="v0.54.0",
                token="token",
                webhook_url="https://discord.test/webhook",
            )
        self.assertEqual(result, "message-54")
        mark.assert_called_once_with(
            repository="owner/repo",
            release=latest,
            tag="v0.54.0",
            token="token",
        )


if __name__ == "__main__":
    unittest.main()
