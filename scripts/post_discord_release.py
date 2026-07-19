#!/usr/bin/env python3
"""Post one GitHub release announcement to a Discord webhook.

The release workflow publishes releases with GitHub's built-in GITHUB_TOKEN.
GitHub deliberately suppresses most follow-up events created by that token, so
the publisher explicitly dispatches the same workflow used for external release
events and manual backfills.
"""

from __future__ import annotations

import argparse
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


def format_message(*, tag: str, name: str, body: str, url: str) -> str:
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

    body = release.get("body") or ""
    if already_announced(body, tag):
        print(f"Discord announcement for {tag} is already recorded; skipping")
        return None

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
    latest_release = fetch_release(repository=repository, tag=tag, token=token)
    try:
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
