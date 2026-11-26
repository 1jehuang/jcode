#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import io
import tempfile
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
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
    def test_linux_selfdev_announcement(self):
        tag = 'selfdev-linux-' + 'ab' * 20
        format_notes = lambda body: MODULE.format_message(tag=tag, name='', body=body, url='https://example.org/download')
        msg = format_notes('## Highlights\n- Captions update as you speak.\n- Configure the push-to-talk key.\n\n<!-- marker -->')
        self.assertIn('**What changed**\n- Captions update as you speak.\n- Configure the push-to-talk key.', msg)
        self.assertIn('curl -fsSL https://jcode.sh/install | bash -s -- --prerelease', msg)
        self.assertTrue(msg.endswith('[Download & release notes](<https://example.org/download>)'))
        self.assertNotIn('marker', msg)
        self.assertNotIn('What changed', format_notes('Older build without highlights.'))
        comparison = 'https://github.com/1jehuang/jcode/compare/' + '0' * 40 + '...' + 'ab' * 20
        self.assertIn(f'[All 19 commits](<{comparison}>)', format_notes(f'## Bundled changes\n\n[View all 19 commits]({comparison})'))
        self.assertLessEqual(len(format_notes('## Highlights\n' + ('- ' + '🙂' * 300 + '\n') * 8).encode('utf-16-le')) // 2, 2000)

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

    def test_linux_selfdev_includes_all_highlights_bundled_commits_and_clean_text(self) -> None:
        base = "0" * 40
        sha = "ab" * 20
        tag = f"selfdev-linux-{sha}"
        compare = f"https://github.com/1jehuang/jcode/compare/{base}...{sha}"
        long_highlight = (
            "Antigravity: support reasoning effort switching (low/medium/high/xhigh/swarm) "
            "via sibling model tiers, default to gemini-4-neon-low, and present Google login "
            "with pooled Gemini, Argon, and Claude quota bars"
        )
        body = (
            f"Portable Linux x86_64 self-dev build of {sha}.\n\n"
            "## Highlights\n\n"
            f"- {long_highlight}\n"
            r"- TUI &amp; diagnostics: \`jcode login\` &amp; auth\_remote improvements @everyone https://example.test/doc" + "\n"
            "- Third highlight\n"
            "- Fourth highlight\n"
            "- Fifth highlight that must not be dropped\n"
            "- Sixth highlight also preserved\n\n"
            "## Bundled changes\n\n"
            f"[View all 14 commits]({compare})\n"
            "- Fifth highlight that must not be dropped\n"
            "- Add scriptable --api-key and --api-key-env login flags\n"
            "- Fix auth\\_remote &amp; model picker ranking\n"
            "- … and 11 more commits in the comparison above.\n"
        )
        msg = MODULE.format_message(
            tag=tag, name="", body=body, url="https://example.org/download"
        )
        self.assertLessEqual(MODULE.utf16_len(msg), MODULE.DISCORD_LIMIT)
        self.assertIn(long_highlight, msg)
        self.assertIn("TUI & diagnostics: `jcode login` & auth_remote improvements ＠everyone <https://example.test/doc>", msg)
        self.assertNotIn("&amp;", msg)
        self.assertNotIn(r"\`", msg)
        self.assertNotIn(r"\_", msg)
        self.assertIn("- Fifth highlight that must not be dropped", msg)
        self.assertIn("- Sixth highlight also preserved", msg)
        self.assertIn("**Bundled commits (14)**", msg)
        self.assertIn("- Add scriptable --api-key and --api-key-env login flags", msg)
        self.assertIn("- Fix auth_remote & model picker ranking", msg)
        # Duplicate bundled commit matching a highlight should not be repeated.
        self.assertEqual(msg.count("Fifth highlight that must not be dropped"), 1)
        self.assertIn("- … and 11 more commits", msg)
        self.assertTrue(msg.endswith(f"[Download & release notes](<https://example.org/download>) · [All 14 commits](<{compare}>)"))

    def test_stale_checked_in_announcement_falls_back_to_release_body(self) -> None:
        import hashlib
        import subprocess
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            discord_dir = root / "discord"
            discord_dir.mkdir()
            changelog_file = root / "v9.0.0.json"
            discord_file = discord_dir / "v9.0.0.json"
            changelog_file.write_text('{"version":"9.0.0","highlights":["Full feature set"]}', encoding="utf-8")

            # 1. Explicit changelog_sha256 mismatch is ignored as stale.
            discord_file.write_text(
                json.dumps({
                    "changelog_sha256": "0" * 64,
                    "messages": ["# jcode v9.0.0\n\nStale single bullet."],
                }),
                encoding="utf-8",
            )
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                self.assertIsNone(MODULE.load_announcement("v9.0.0"))

            # Matching changelog_sha256 is accepted.
            matching_sha = hashlib.sha256(changelog_file.read_bytes()).hexdigest()
            discord_file.write_text(
                json.dumps({
                    "changelog_sha256": matching_sha,
                    "messages": ["# jcode v9.0.0\n\nComplete summary."],
                }),
                encoding="utf-8",
            )
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                self.assertEqual(
                    MODULE.load_announcement("v9.0.0"),
                    ["# jcode v9.0.0\n\nComplete summary."],
                )

            # 2. Legacy plain-list announcement is rejected when git history shows
            # changelog/v9.0.0.json was updated in a strictly newer commit.
            def git(*args: str) -> None:
                subprocess.run(
                    ["git", *args],
                    cwd=root,
                    check=True,
                    capture_output=True,
                    text=True,
                )

            git("init", "-q")
            git("config", "user.name", "Fixture")
            git("config", "user.email", "fixture@example.invalid")
            discord_file.write_text('["# jcode v9.0.0\\n\\nOld summary."]', encoding="utf-8")
            git("add", ".")
            git("commit", "-qm", "initial release notes")
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                self.assertEqual(
                    MODULE.load_announcement("v9.0.0"),
                    ["# jcode v9.0.0\n\nOld summary."],
                )

            # Update only changelog/v9.0.0.json in a newer commit.
            changelog_file.write_text('{"version":"9.0.0","highlights":["Updated full feature set"]}', encoding="utf-8")
            git("add", "v9.0.0.json")
            git("commit", "-qm", "update changelog without updating discord override")
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                self.assertIsNone(MODULE.load_announcement("v9.0.0"))

            # Updating discord override in a newer commit makes it valid again.
            discord_file.write_text('["# jcode v9.0.0\\n\\nUpdated summary."]', encoding="utf-8")
            git("add", "discord/v9.0.0.json")
            git("commit", "-qm", "sync discord override")
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                self.assertEqual(
                    MODULE.load_announcement("v9.0.0"),
                    ["# jcode v9.0.0\n\nUpdated summary."],
                )

    def test_stable_message_strips_discord_markers_and_truncates_utf16_on_line_boundary(self) -> None:
        url = "https://github.com/1jehuang/jcode/releases/tag/v1.0.9"
        body = (
            "### Highlights\n"
            "- Feature A 🙂\n\n"
            "<!-- jcode-discord-part:v1.0.9:deadbeef:1:sent:123 -->\n"
            "<!-- jcode-discord-announced:v1.0.9 -->\n"
        )
        short = MODULE.format_message(tag="v1.0.9", name="v1.0.9", body=body, url=url)
        self.assertNotIn("jcode-discord-", short)
        self.assertIn("- Feature A 🙂", short)

        long_body = "\n".join(f"- Bullet {i}: " + ("🙂" * 40) for i in range(40))
        truncated = MODULE.format_message(tag="v1.0.9", name="v1.0.9", body=long_body, url=url)
        self.assertLessEqual(MODULE.utf16_len(truncated), MODULE.DISCORD_LIMIT)
        self.assertTrue(truncated.endswith(f"\n… (full notes: <{url}>)"))
        # Should end on a full bullet line before the suffix, not mid-emoji/mid-bullet.
        before_suffix = truncated.removesuffix(f"\n… (full notes: <{url}>)")
        self.assertTrue(before_suffix.endswith("🙂" * 40))

    def test_checked_in_discord_announcements_are_valid_and_not_stale(self) -> None:
        if not MODULE.ANNOUNCEMENT_DIR.exists():
            return
        for path in sorted(MODULE.ANNOUNCEMENT_DIR.glob("*.json")):
            tag = path.stem
            with self.subTest(tag=tag):
                messages = MODULE.load_announcement(tag)
                self.assertIsNotNone(
                    messages,
                    f"{path} is stale relative to changelog/{tag}.json; update or remove it",
                )

    def test_preview_unescapes_markdown_escapes_while_wrapping_urls(self) -> None:
        body = (
            "## Commits\n\n"
            r"- [abc12345](https://example.test/c) Fix \`--provider\` and foo\_bar \[beta\] \(see https://x.test/y\)"
            "\n"
        )
        msg = MODULE.format_message(
            tag="v1.0.0-preview.99",
            name="Preview",
            body=body,
            url="https://example.test/releases/preview.99",
        )
        self.assertIn("- Fix `--provider` and foo_bar [beta] (see <https://x.test/y>)", msg)
        self.assertNotIn(r"\`", msg)
        self.assertNotIn(r"\_", msg)
        self.assertNotIn(r"\[", msg)

    def test_stable_full_changelog_url_is_wrapped_to_suppress_embeds(self) -> None:
        body = (
            "### Fixes\n\n- Fix a bug.\n\n"
            "**Full changelog**: https://github.com/1jehuang/jcode/compare/v1.0.1...v1.0.3\n"
        )
        msg = MODULE.format_message(
            tag="v1.0.3",
            name="v1.0.3",
            body=body,
            url="https://github.com/1jehuang/jcode/releases/tag/v1.0.3",
        )
        self.assertIn(
            "**Full changelog**: <https://github.com/1jehuang/jcode/compare/v1.0.1...v1.0.3>",
            msg,
        )

    def test_selfdev_single_commit_link_and_word_safe_truncation(self) -> None:
        sha = "c" * 40
        prev = "d" * 40
        tag = f"selfdev-linux-{sha}"
        body = (
            "## Highlights\n\n"
            "- " + " ".join(f"`token-{i}`" for i in range(60)) + "\n\n"
            "## Bundled changes\n\n"
            f"[View all 1 commit](https://github.com/1jehuang/jcode/compare/{prev}...{sha})\n"
            "- fix: single commit\n"
        )
        msg = MODULE.format_message(
            tag=tag,
            name=tag,
            body=body,
            url=f"https://github.com/1jehuang/jcode/releases/tag/{tag}",
        )
        self.assertIn(
            f"[1 commit](<https://github.com/1jehuang/jcode/compare/{prev}...{sha}>)",
            msg,
        )
        highlight_line = msg.split("**What changed**\n", 1)[1].splitlines()[0]
        self.assertEqual(highlight_line.count("`") % 2, 0)
        self.assertNotRegex(highlight_line, r"token-\d*…$")

        # Pre-truncated legacy GitHub body with an unclosed backtick before ellipsis stays balanced.
        legacy = MODULE.clean_selfdev_bullet(r"providers (\`https\`, \`grok-buil…")
        self.assertEqual(legacy, "providers (`https`…")
        self.assertEqual(legacy.count("`") % 2, 0)

    def test_selfdev_header_includes_version_from_title_or_cargo_toml(self) -> None:
        sha = "1" * 40
        tag = f"selfdev-linux-{sha}"
        url = f"https://github.com/1jehuang/jcode/releases/tag/{tag}"
        msg = MODULE.format_message(
            tag=tag,
            name=f"jcode v1.0.3-dev · Linux prerelease · {sha[:9]}",
            body="## Highlights\n- Stream thoughts live\n",
            url=url,
        )
        self.assertEqual(msg.splitlines()[0], "## jcode v1.0.3-dev · Linux prerelease")

        # When title has no version, fall back to Cargo.toml at the commit SHA.
        head_sha = MODULE.subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        head_tag = f"selfdev-linux-{head_sha}"
        head_msg = MODULE.format_message(
            tag=head_tag,
            name=f"jcode Linux prerelease · {head_sha[:9]}",
            body="## Highlights\n- Stream thoughts live\n",
            url=f"https://github.com/1jehuang/jcode/releases/tag/{head_tag}",
        )
        self.assertRegex(
            head_msg.splitlines()[0],
            r"^## jcode v\d+\.\d+\.\d+-dev · Linux prerelease$",
        )

    def test_selfdev_splits_visual_and_non_visual_highlights(self) -> None:
        sha = "2" * 40
        tag = f"selfdev-linux-{sha}"
        msg = MODULE.format_message(
            tag=tag,
            name=f"jcode v1.0.3-dev · Linux prerelease · {sha[:9]}",
            body=(
                "## Highlights\n"
                "- Unify collapsible OAuth progress card in the TUI onboarding flow\n"
                "- Stream Gemini thought summaries live over SSE\n"
            ),
            url=f"https://github.com/1jehuang/jcode/releases/tag/{tag}",
        )
        self.assertIn("**Visual & UI**\n- Unify collapsible OAuth progress card", msg)
        self.assertIn("**What changed**\n- Stream Gemini thought summaries live over SSE", msg)

    def test_announcement_supports_multipart_image_attachments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            discord_dir = root / "discord"
            discord_dir.mkdir()
            png_path = discord_dir / "v1.0.4-before-after.png"
            png_bytes = b"\x89PNG\r\n\x1a\nfake-png-bytes"
            png_path.write_bytes(png_bytes)
            (discord_dir / "v1.0.4.json").write_text(
                json.dumps(
                    {
                        "messages": [
                            {
                                "content": "## v1.0.4\n\n**Visual & UI**\n- Cleaner header",
                                "attachments": ["v1.0.4-before-after.png"],
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                parts = MODULE.load_announcement_parts("v1.0.4")
            self.assertIsNotNone(parts)
            assert parts is not None
            self.assertEqual(len(parts), 1)
            self.assertEqual(parts[0][1], [png_path.resolve()])

            body, content_type = MODULE.build_discord_multipart(
                payload={"content": parts[0][0], "allowed_mentions": {"parse": []}},
                attachments=parts[0][1],
            )
            self.assertTrue(content_type.startswith("multipart/form-data; boundary="))
            self.assertIn(b'name="payload_json"', body)
            self.assertIn(b'name="files[0]"; filename="v1.0.4-before-after.png"', body)
            self.assertIn(png_bytes, body)

            # Reject attachment paths that escape changelog/discord/.
            (discord_dir / "v1.0.5.json").write_text(
                json.dumps({"messages": ["hi"], "attachments": ["../escape.png"]}),
                encoding="utf-8",
            )
            with mock.patch.object(MODULE, "ANNOUNCEMENT_DIR", discord_dir):
                with self.assertRaisesRegex(RuntimeError, "escapes"):
                    MODULE.load_announcement_parts("v1.0.5")

    def test_long_stable_release_auto_splits_via_multipart(self) -> None:
        url = "https://github.com/1jehuang/jcode/releases/tag/v0.96.0"
        body = (
            "### Highlights\n\n"
            + "\n".join(f"- Highlight item {i}: " + ("important detail " * 10) for i in range(8))
            + "\n\n### Fixes\n\n"
            + "\n".join(f"- Fix item {i}: " + ("bugfix explanation " * 10) for i in range(10))
            + "\n\n**Full changelog**: https://github.com/1jehuang/jcode/compare/v0.95.0...v0.96.0\n"
        )
        parts = MODULE.format_messages(tag="v0.96.0", name="v0.96.0", body=body, url=url)
        self.assertGreater(len(parts), 1)
        self.assertLessEqual(len(parts), MODULE.MAX_AUTO_MULTIPART)
        for part in parts:
            self.assertLessEqual(MODULE.utf16_len(part), MODULE.DISCORD_LIMIT)
            # Never leave an orphan heading at the very end of a chunk.
            self.assertFalse(part.splitlines()[-1].startswith("### "))
        joined = "\n".join(parts)
        self.assertIn("Highlight item 0:", joined)
        self.assertIn("Fix item 9:", joined)
        self.assertIn("<https://github.com/1jehuang/jcode/compare/v0.95.0...v0.96.0>", joined)

        # Verify announce_release uses receipt-backed multipart delivery for long stable releases.
        release, posts, _, request = self.multipart_http()
        release["body"] = body
        release["name"] = "v0.96.0"
        release["html_url"] = url
        with (
            mock.patch.object(MODULE, "load_announcement", return_value=None),
            mock.patch.object(MODULE.urllib.request, "urlopen", side_effect=request),
        ):
            MODULE.announce_release(
                repository="1jehuang/jcode",
                tag="v0.96.0",
                token="fake",
                webhook_url="https://discord.test/webhook",
            )
        self.assertEqual(len(posts), len(parts))
        self.assertIn(MODULE.announcement_marker("v0.96.0"), release["body"])

        # Massive body caps cleanly at MAX_AUTO_MULTIPART with a full-notes link.
        huge_body = "\n".join(f"- Huge item {i}: " + ("x" * 180) for i in range(120))
        capped = MODULE.format_messages(tag="v0.96.0", name="v0.96.0", body=huge_body, url=url)
        self.assertEqual(len(capped), MODULE.MAX_AUTO_MULTIPART)
        self.assertTrue(capped[-1].endswith(f"\n… (full notes: <{url}>)"))
        self.assertTrue(all(MODULE.utf16_len(p) <= MODULE.DISCORD_LIMIT for p in capped))

    def test_discover_selfdev_attachments_finds_only_new_images_in_range(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=repo, check=True)

            discord_dir = repo / "changelog" / "discord"
            discord_dir.mkdir(parents=True)
            (discord_dir / "old.png").write_bytes(b"\x89PNG\r\n\x1a\nold")
            subprocess.run(["git", "add", "."], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-q", "-m", "initial"], cwd=repo, check=True)
            prev_sha = subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=repo, text=True
            ).strip()

            (discord_dir / "selfdev-login.png").write_bytes(b"\x89PNG\r\n\x1a\nnew")
            (discord_dir / "notes.json").write_text("{}", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-q", "-m", "ui change"], cwd=repo, check=True)
            head_sha = subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=repo, text=True
            ).strip()

            body = f"Compare: https://github.com/1jehuang/jcode/compare/{prev_sha}...{head_sha}"
            found = MODULE.discover_selfdev_attachments(
                tag=f"selfdev-linux-{head_sha}",
                body=body,
                repo_root=repo,
            )
            self.assertEqual([p.name for p in found], ["selfdev-login.png"])


if __name__ == "__main__":
    unittest.main()
