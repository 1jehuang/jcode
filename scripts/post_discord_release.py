#!/usr/bin/env python3
"""Post one GitHub release announcement to a Discord webhook.

The release workflow publishes releases with GitHub's built-in GITHUB_TOKEN.
GitHub deliberately suppresses most follow-up events created by that token, so
the publisher explicitly dispatches the same workflow used for external release
events and manual backfills.

Optional changelog/discord/<tag>.json files contain a nonempty JSON array of
content strings, each at most 2000 UTF-16 code units. They are sent verbatim.
Multipart delivery checkpoints each part in the release body. An unresolved
`pending` checkpoint requires manual reconciliation: confirm whether Discord
received that part, then replace its state with `sent:<message-id>` if delivered,
or remove that checkpoint only after confirming no message was delivered.
Never simply rerun or delete a pending checkpoint after an uncertain delivery.
The workflow's per-tag concurrency group must serialize runs of this script.
"""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


DISCORD_LIMIT = 2_000
MARKER_PREFIX = "jcode-discord-announced"
PLATFORM_BLOCK = re.compile(
    r"\n?<!-- jcode-platform-availability:start -->.*?"
    r"<!-- jcode-platform-availability:end -->\n?",
    flags=re.DOTALL,
)


def announcement_marker(tag: str) -> str:
    return f"<!-- {MARKER_PREFIX}:{tag} -->"


def already_announced(body: str, tag: str) -> bool:
    return announcement_marker(tag) in body


def release_notes_for_discord(body: str) -> str:
    body = PLATFORM_BLOCK.sub("\n", body)
    return re.sub(r"\n{3,}", "\n\n", body).strip()


COMMIT_LINE = re.compile(r"^- \[[0-9a-f]{4,40}\]\([^)]*\) (.+)$")


def preview_commit_lines(notes: str) -> list[str]:
    """Return `- subject` bullets from the generated `## Commits` section."""
    section = re.search(r"^## Commits[^\S\n]*\n(.*?)(?=^## |\Z)", notes,
                        flags=re.MULTILINE | re.DOTALL)
    if not section:
        return []
    lines = []
    for raw in section.group(1).splitlines():
        match = COMMIT_LINE.match(raw.strip())
        if match:
            # Neutralize any stray URL so the release link stays the only one.
            subject = re.sub(r"(?<!<)https?://[^\s<>()]+", r"<\g<0>>", match.group(1))
            lines.append(f"- {subject}")
    return lines


def format_message(*, tag: str, name: str, body: str, url: str) -> str:
    if re.fullmatch(r"v1\.0\.0-preview\.[1-9][0-9]*", tag):
        # An arbitrary URL cannot both fit Discord and remain intact. Fall back
        # to the project's downloads page instead of emitting a broken link.
        download_url = url if len(url) <= 256 else "https://github.com/1jehuang/jcode/releases"
        # Minimal format: one header line with the single release link, then
        # the bare commit titles. No setup prose, no per-commit links.
        header = f"**jcode {tag[:120]}** (preview) {download_url}\n"
        commits = preview_commit_lines(release_notes_for_discord(body))
        if not commits:
            return header.rstrip()
        lines: list[str] = []
        used = len(header)
        for index, line in enumerate(commits):
            more = f"… {len(commits) - index} more"
            # Always reserve room for the overflow line unless this is the last.
            reserve = 0 if index == len(commits) - 1 else len(more) + 1
            if used + len(line) + 1 + reserve > DISCORD_LIMIT:
                lines.append(more)
                break
            lines.append(line)
            used += len(line) + 1
        return header + "\n".join(lines)
    title = f"## {tag}" + (f" — {name}" if name and name != tag else "")
    notes = release_notes_for_discord(body)
    message = title if not notes else f"{title}\n{notes}"
    suffix = f"\n… (full notes: <{url}>)"
    if len(message) > DISCORD_LIMIT:
        message = message[: DISCORD_LIMIT - len(suffix)] + suffix
    return message


def github_request(
    url: str,
    *,
    token: str,
    method: str = "GET",
    payload: dict[str, Any] | None = None,
) -> dict[str, Any]:
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": "jcode-release-bot/2.0",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def fetch_release(*, repository: str, tag: str, token: str) -> dict[str, Any]:
    quoted_tag = urllib.parse.quote(tag, safe="")
    return github_request(
        f"https://api.github.com/repos/{repository}/releases/tags/{quoted_tag}",
        token=token,
    )


def discord_webhook_url(webhook_url: str) -> str:
    parts = urllib.parse.urlsplit(webhook_url)
    query = urllib.parse.parse_qsl(parts.query, keep_blank_values=True)
    if not any(key == "wait" for key, _ in query):
        query.append(("wait", "true"))
    return urllib.parse.urlunsplit(
        (
            parts.scheme,
            parts.netloc,
            parts.path,
            urllib.parse.urlencode(query),
            parts.fragment,
        )
    )


def post_to_discord(*, webhook_url: str, content: str) -> dict[str, Any]:
    request = urllib.request.Request(
        discord_webhook_url(webhook_url),
        data=json.dumps(
            {
                "content": content,
                # Release notes are generated from commit subjects. Never let
                # them turn @everyone or user-looking text into real mentions.
                "allowed_mentions": {"parse": []},
            }
        ).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "User-Agent": "jcode-release-bot/2.0",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        result = json.load(response)
    if not result.get("id"):
        raise RuntimeError("Discord accepted the webhook but returned no message id")
    return result


def mark_release_announced(
    *, repository: str, release: dict[str, Any], tag: str, token: str
) -> None:
    body = release.get("body") or ""
    marker = announcement_marker(tag)
    if marker in body:
        return
    updated_body = f"{body.rstrip()}\n\n{marker}\n"
    github_request(
        f"https://api.github.com/repos/{repository}/releases/{release['id']}",
        token=token,
        method="PATCH",
        payload={"body": updated_body},
    )


ANNOUNCEMENT_DIR = Path(__file__).resolve().parent.parent / "changelog" / "discord"


def load_announcement(tag: str) -> list[str] | None:
    # Tags must never escape the checked-in announcement directory.
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", tag):
        return None
    path = ANNOUNCEMENT_DIR / f"{tag}.json"
    if not path.exists():
        return None
    try:
        messages = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(messages, list) or not messages:
            raise ValueError("expected a nonempty array of content strings")
        for index, content in enumerate(messages):
            if not isinstance(content, str) or not content.strip():
                raise ValueError(f"message {index + 1} must be a nonempty string")
            if len(content.encode("utf-16-le")) // 2 > DISCORD_LIMIT:
                raise ValueError(f"message {index + 1} exceeds 2000 UTF-16 units")
    except (OSError, ValueError, UnicodeError) as error:
        raise RuntimeError(f"Invalid Discord announcement {path}: {error}") from error
    return messages


def multipart_marker(tag: str, digest: str, index: int, state: str) -> str:
    return f"<!-- jcode-discord-part:{tag}:{digest}:{index}:{state} -->"


def announce_multipart(*, repository: str, tag: str, token: str,
                       webhook_url: str, messages: list[str]) -> str | None:
    digest = hashlib.sha256(json.dumps(messages, ensure_ascii=True).encode()).hexdigest()
    last_id = None

    def latest() -> dict[str, Any]:
        release = fetch_release(repository=repository, tag=tag, token=token)
        body = release.get("body") or ""
        if release.get("draft"):
            raise RuntimeError(f"Refusing to announce draft release {tag}")
        parts = re.findall(r"<!-- jcode-discord-part:" + re.escape(tag)
                           + r":([^:]+):(\d+):([^ ]+) -->", body)
        for saved_digest, _, state in parts:
            if saved_digest != digest:
                raise RuntimeError("Announcement content changed after posting began")
            if state == "pending":
                raise RuntimeError("Unresolved Discord pending delivery. Reconcile manually before retrying")
        return release

    def persist(release: dict[str, Any], body: str) -> None:
        github_request(
            f"https://api.github.com/repos/{repository}/releases/{release['id']}",
            token=token, method="PATCH", payload={"body": body},
        )

    for index, content in enumerate(messages, 1):
        release = latest()
        body = release.get("body") or ""
        if already_announced(body, tag):
            return last_id
        receipt_prefix = multipart_marker(tag, digest, index, "sent:")[:-4]
        if receipt_prefix in body:
            continue
        pending = multipart_marker(tag, digest, index, "pending")
        # A durable intent checkpoint closes the lost-response duplicate window.
        persist(release, f"{body.rstrip()}\n\n{pending}\n")
        try:
            message = post_to_discord(webhook_url=webhook_url, content=content)
            last_id = str(message["id"])
            # Fetch again to preserve edits made during webhook delivery.
            release = fetch_release(repository=repository, tag=tag, token=token)
            body = release.get("body") or ""
            if pending not in body:
                raise RuntimeError("Pending checkpoint disappeared during delivery")
            receipt = multipart_marker(tag, digest, index, f"sent:{last_id}")
            persist(release, body.replace(pending, receipt))
        except Exception as error:
            # Do not report a retryable workflow failure after a possible public
            # side effect. The durable pending checkpoint blocks future sends.
            print(f"::warning::Partial Discord announcement {tag}, part {index}/{len(messages)}: "
                  f"delivery or receipt uncertain ({error}). STOP: manually reconcile the pending "
                  "checkpoint with Discord before retrying.", file=sys.stderr)
            return last_id
    release = latest()
    body = release.get("body") or ""
    for index in range(1, len(messages) + 1):
        if multipart_marker(tag, digest, index, "sent:")[:-4] not in body:
            raise RuntimeError(f"Missing Discord receipt for part {index}; refusing final marker")
    mark_release_announced(repository=repository, release=release, tag=tag, token=token)
    return last_id


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default=os.environ.get("GITHUB_REF_NAME"))
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"))
    return parser.parse_args()


def required(value: str | None, description: str) -> str:
    if value:
        return value
    raise SystemExit(f"Missing {description}")


def announce_release(
    *, repository: str, tag: str, token: str, webhook_url: str
) -> str | None:
    release = fetch_release(repository=repository, tag=tag, token=token)
    if release.get("draft"):
        raise RuntimeError(f"Refusing to announce draft release {tag}")
    if (re.fullmatch(r"v1\.0\.0-preview\.[1-9][0-9]*", tag)
            and release.get("prerelease") is not True):
        raise RuntimeError(f"Refusing preview announcement for non-prerelease {tag}")

    body = release.get("body") or ""
    if already_announced(body, tag):
        print(f"Discord announcement for {tag} is already recorded; skipping")
        return None

    messages = load_announcement(tag)
    if messages is not None:
        return announce_multipart(repository=repository, tag=tag, token=token,
                                  webhook_url=webhook_url, messages=messages)

    content = format_message(
        tag=tag,
        name=release.get("name") or tag,
        body=body,
        url=release["html_url"],
    )
    message = post_to_discord(webhook_url=webhook_url, content=content)
    message_id = str(message["id"])
    print(f"Posted {tag} to Discord as message {message_id}")

    # Re-fetch immediately before patching so an edit made while the Discord
    # request was in flight is not overwritten with the initially fetched body.
    try:
        latest_release = fetch_release(repository=repository, tag=tag, token=token)
        mark_release_announced(
            repository=repository, release=latest_release, tag=tag, token=token
        )
    except Exception as error:  # noqa: BLE001 - best-effort after public side effect
        # A marker write failure must not turn a successful public post into a
        # failed workflow that an operator is likely to rerun and duplicate.
        print(
            f"::warning::Discord post succeeded, but the release marker failed: {error}",
            file=sys.stderr,
        )
    return message_id


def main() -> int:
    args = parse_args()
    tag = required(args.tag, "release tag (--tag or GITHUB_REF_NAME)")
    repository = required(
        args.repository, "GitHub repository (--repository or GITHUB_REPOSITORY)"
    )
    token = required(
        os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN"), "GH_TOKEN"
    )
    webhook_url = required(
        os.environ.get("DISCORD_RELEASE_WEBHOOK"), "DISCORD_RELEASE_WEBHOOK"
    )

    try:
        announce_release(
            repository=repository,
            tag=tag,
            token=token,
            webhook_url=webhook_url,
        )
        return 0
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        print(f"HTTP {error.code} while announcing {tag}: {detail}", file=sys.stderr)
        return 1
    except (RuntimeError, urllib.error.URLError) as error:
        print(f"Could not announce {tag}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
