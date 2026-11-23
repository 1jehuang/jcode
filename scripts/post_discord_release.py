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
import html
import mimetypes
from pathlib import Path
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from typing import Any


DISCORD_LIMIT = 2_000
MARKER_PREFIX = "jcode-discord-announced"
PLATFORM_BLOCK = re.compile(
    r"\n?<!-- jcode-platform-availability:start -->.*?"
    r"<!-- jcode-platform-availability:end -->\n?",
    flags=re.DOTALL,
)
DISCORD_COMMENT_MARKER = re.compile(r"\n?<!-- jcode-discord-[^>]*-->\n?")


def announcement_marker(tag: str) -> str:
    return f"<!-- {MARKER_PREFIX}:{tag} -->"


def already_announced(body: str, tag: str) -> bool:
    return announcement_marker(tag) in body


def release_notes_for_discord(body: str) -> str:
    body = PLATFORM_BLOCK.sub("\n", body)
    body = DISCORD_COMMENT_MARKER.sub("\n", body)
    # Suppress Discord link preview cards on bare Full changelog compare links.
    body = re.sub(
        r"^(\*\*Full changelog\*\*:\s+)(https?://[^\s<>]+)$",
        r"\1<\2>",
        body,
        flags=re.MULTILINE,
    )
    return re.sub(r"\n{3,}", "\n\n", body).strip()


COMMIT_LINE = re.compile(r"^- \[[0-9a-f]{4,40}\]\([^)]*\) (.+)$")
ESCAPED_MARKDOWN = re.compile(r"\\([\\`*_{}\[\]()<>#~|+=.-])")
RAW_URL = re.compile(r"(?<!<)https?://[^\s<>()]+")
MORE_COMMITS_BULLET = re.compile(r"^…\s+and\s+\d+\s+more\s+commits?\b", re.I)
VISUAL_CHANGE_RE = re.compile(
    r"\b("
    r"tui|ui|ux|visual|design|layout|theme|palette|color|render(?:ing|er|ed)?|"
    r"banner|header|footer|sidebar|widget|status\s+bar|picker|modal|overlay|"
    r"card|markdown|mermaid|spacing|align(?:ment)?|wrap(?:ping)?|truncat(?:e|ion)|"
    r"animation|spinner|onboarding|progress\s+card|scroll(?:bar|ing)?|diff\s+view|"
    r"pane|border|font|icon|glyph|screenshot"
    r")\b",
    re.I,
)
MAX_AUTO_MULTIPART = 4


def is_visual_change(bullet: str) -> bool:
    return bool(VISUAL_CHANGE_RE.search(bullet))


def utf16_len(text: str) -> int:
    return len(text.encode("utf-16-le")) // 2


def truncate_utf16(text: str, max_units: int, suffix: str = "…") -> str:
    if utf16_len(text) <= max_units:
        return text
    budget = max(0, max_units - utf16_len(suffix))
    out: list[str] = []
    used = 0
    for ch in text:
        width = utf16_len(ch)
        if used + width > budget:
            break
        out.append(ch)
        used += width
    prefix = "".join(out)
    if " " in prefix:
        last_space = prefix.rfind(" ")
        if utf16_len(prefix[:last_space]) >= budget // 2:
            prefix = prefix[:last_space]
    if prefix.count("`") % 2 == 1:
        last_tick = prefix.rfind("`")
        if last_tick > 0:
            prefix = prefix[:last_tick]
    prefix = prefix.rstrip(" ,;:([{-")
    if not prefix:
        prefix = "".join(out).rstrip()
    return prefix + suffix


def clean_selfdev_bullet(raw: str) -> str:
    text = html.unescape(raw.strip())
    text = ESCAPED_MARKDOWN.sub(r"\1", text)
    text = text.replace("@", "＠")
    text = RAW_URL.sub(r"<\g<0>>", text)
    text = " ".join(text.split())
    # Older GitHub releases may contain bullets truncated mid-backtick span;
    # strip a dangling unclosed backtick before the ellipsis so Discord markdown stays valid.
    if text.endswith("…") and text.count("`") % 2 == 1:
        last_tick = text.rfind("`")
        if last_tick > 0:
            text = text[:last_tick].rstrip(" ,;:([{-") + "…"
    return text


def truncate_stable_message(message: str, url: str) -> str:
    if utf16_len(message) <= DISCORD_LIMIT:
        return message
    suffix = f"\n… (full notes: <{url}>)"
    budget = max(0, DISCORD_LIMIT - utf16_len(suffix))
    out: list[str] = []
    used = 0
    for ch in message:
        width = utf16_len(ch)
        if used + width > budget:
            break
        out.append(ch)
        used += width
    prefix = "".join(out)
    last_nl = prefix.rfind("\n")
    if last_nl > 0 and utf16_len(prefix[:last_nl]) >= budget // 2:
        prefix = prefix[:last_nl].rstrip()
    return prefix + suffix


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
            # Unescape GitHub markdown escapes from preview_notes.py while keeping
            # any stray URL wrapped so the release link stays the only active one.
            subject = ESCAPED_MARKDOWN.sub(r"\1", html.unescape(match.group(1)))
            subject = RAW_URL.sub(r"<\g<0>>", subject)
            lines.append(f"- {subject}")
    return lines


def resolve_selfdev_version(*, sha: str, name: str) -> str | None:
    if match := re.search(r"\bv\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\b", name):
        return match.group(0)
    try:
        raw = subprocess.check_output(
            ["git", "show", f"{sha}:Cargo.toml"],
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    in_package = False
    for line in raw.splitlines():
        trimmed = line.strip()
        if trimmed.startswith("["):
            in_package = trimmed == "[package]"
            continue
        if in_package and (ver := re.match(r'^version\s*=\s*"([^"]+)"', trimmed)):
            return f"v{ver.group(1)}-dev"
    return None


def format_message(*, tag: str, name: str, body: str, url: str) -> str:
    if match := re.fullmatch(r"selfdev-linux-([0-9a-f]{40})", tag):
        version = resolve_selfdev_version(sha=match[1], name=name)
        heading = (
            f"## jcode {version} · Linux prerelease"
            if version
            else "## jcode · Linux prerelease"
        )
        header = [heading, f"Linux x86_64 · Build `{match[1][:9]}`"]
        links = f"[Download & release notes](<{url}>)"
        comparison = re.search(
            r"^\[View all ([0-9]+) commits?\]\(https://github\.com/1jehuang/jcode/compare/([0-9a-f]{40})\.\.\.([0-9a-f]{40})\)$",
            body,
            re.M,
        )
        total_commits: int | None = None
        if comparison and comparison[3] == match[1]:
            total_commits = int(comparison[1])
            label = "1 commit" if total_commits == 1 else f"All {comparison[1]} commits"
            links += f" · [{label}](<https://github.com/1jehuang/jcode/compare/{comparison[2]}...{match[1]}>)"

        def extract_bullets(heading: str) -> list[str]:
            section = re.search(
                rf"^## {re.escape(heading)}[ \t]*\n(.*?)(?=^#|^<!--|\Z)",
                body,
                re.M | re.S,
            )
            if not section:
                return []
            extracted: list[str] = []
            for raw in re.findall(r"^[-*] (\S[^\n]*)", section[1], re.M):
                cleaned = clean_selfdev_bullet(raw)
                if cleaned and not MORE_COMMITS_BULLET.match(cleaned):
                    extracted.append(cleaned)
            return extracted

        highlights = extract_bullets("Highlights")
        bundled_raw = extract_bullets("Bundled changes")
        seen_highlights = {item.casefold() for item in highlights}
        bundled = [item for item in bundled_raw if item.casefold() not in seen_highlights]
        deduped_count = len(bundled_raw) - len(bundled)

        install_hint = "Switch & live-update: `curl -fsSL https://jcode.sh/install | bash -s -- --prerelease`"
        footer = ["", install_hint, links]
        lines = list(header)
        footer_units = utf16_len("\n".join(footer)) + 1

        def remaining_units() -> int:
            return DISCORD_LIMIT - utf16_len("\n".join(lines)) - footer_units

        def append_highlight_section(title: str, items: list[str]) -> None:
            if not items:
                return
            section_overhead = utf16_len(f"\n\n{title}")
            if remaining_units() <= section_overhead + 16:
                return
            lines.extend(["", title])
            omitted = 0
            for idx, item in enumerate(items):
                text = truncate_utf16(item, 360)
                bullet = f"- {text}"
                more_left = len(items) - idx - 1
                reserve = (
                    utf16_len(f"\n- … and {more_left} more")
                    if more_left > 0
                    else 0
                )
                if utf16_len(bullet) + 1 + reserve > remaining_units():
                    if idx == 0:
                        avail = remaining_units() - 3 - reserve
                        if avail > 10:
                            lines.append("- " + truncate_utf16(text, avail))
                            continue
                    omitted = len(items) - idx
                    break
                lines.append(bullet)
            if omitted > 0:
                more_line = f"- … and {omitted} more"
                if utf16_len(more_line) + 1 <= remaining_units():
                    lines.append(more_line)

        if highlights:
            visual_highlights = [item for item in highlights if is_visual_change(item)]
            other_highlights = [item for item in highlights if not is_visual_change(item)]
            if visual_highlights and other_highlights:
                append_highlight_section("**Visual & UI**", visual_highlights)
                append_highlight_section("**What changed**", other_highlights)
            elif visual_highlights:
                append_highlight_section("**Visual & UI**", visual_highlights)
            else:
                append_highlight_section("**What changed**", other_highlights)

        if bundled:
            heading = (
                f"**Bundled commits ({total_commits})**"
                if total_commits is not None and total_commits > 1
                else "**Bundled commits**"
            )
            section_overhead = utf16_len(f"\n\n{heading}")
            if remaining_units() > section_overhead + 30:
                section_lines = ["", heading]
                added = 0
                for idx, item in enumerate(bundled):
                    text = truncate_utf16(item, 160)
                    bullet = f"- {text}"
                    remaining_in_list = len(bundled) - (idx + 1)
                    if total_commits is not None:
                        unshown_after = max(
                            remaining_in_list,
                            total_commits - (idx + 1) - deduped_count,
                        )
                    else:
                        unshown_after = remaining_in_list
                    noun = "commit" if unshown_after == 1 else "commits"
                    reserve = (
                        utf16_len(f"\n- … and {unshown_after} more {noun}")
                        if unshown_after > 0
                        else 0
                    )
                    candidate_units = (
                        utf16_len("\n".join(lines + section_lines + [bullet]))
                        + footer_units
                        + reserve
                    )
                    if candidate_units > DISCORD_LIMIT:
                        break
                    section_lines.append(bullet)
                    added += 1
                if added > 0:
                    lines.extend(section_lines)
                    if total_commits is not None:
                        unshown = max(
                            len(bundled) - added,
                            total_commits - added - deduped_count,
                        )
                    else:
                        unshown = len(bundled) - added
                    if unshown > 0:
                        noun = "commit" if unshown == 1 else "commits"
                        more_line = f"- … and {unshown} more {noun}"
                        if utf16_len(more_line) + 1 <= remaining_units():
                            lines.append(more_line)

        lines.extend(footer)
        return "\n".join(lines)
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
        used = utf16_len(header)
        for index, line in enumerate(commits):
            more = f"… {len(commits) - index} more"
            # Always reserve room for the overflow line unless this is the last.
            reserve = 0 if index == len(commits) - 1 else utf16_len(more) + 1
            if used + utf16_len(line) + 1 + reserve > DISCORD_LIMIT:
                lines.append(more)
                break
            lines.append(line)
            used += utf16_len(line) + 1
        return header + "\n".join(lines)
    title = f"## {tag}" + (f" — {name}" if name and name != tag else "")
    notes = release_notes_for_discord(body)
    message = title if not notes else f"{title}\n{notes}"
    return truncate_stable_message(message, url)


def split_stable_message(message: str, url: str, *, max_parts: int = MAX_AUTO_MULTIPART) -> list[str]:
    if utf16_len(message) <= DISCORD_LIMIT:
        return [message]
    lines = message.splitlines()
    chunks: list[str] = []
    current: list[str] = []

    def joined(items: list[str]) -> str:
        return "\n".join(items).strip()

    idx = 0
    while idx < len(lines):
        line = lines[idx]
        # Keep a section heading attached to the first non-empty line below it.
        group = [line]
        if re.match(r"^#{1,6}\s+\S", line):
            look = idx + 1
            while look < len(lines) and lines[look].strip() == "":
                group.append(lines[look])
                look += 1
            if look < len(lines) and not re.match(r"^#{1,6}\s+\S", lines[look]):
                group.append(lines[look])
                look += 1
            next_idx = look
        else:
            next_idx = idx + 1

        candidate = current + group
        if utf16_len(joined(candidate)) <= DISCORD_LIMIT:
            current = candidate
            idx = next_idx
            continue

        if current:
            chunks.append(joined(current))
            current = []
            if len(chunks) == max_parts - 1:
                remainder = "\n".join(lines[idx:]).strip()
                if remainder:
                    chunks.append(truncate_stable_message(remainder, url))
                return [c for c in chunks if c]
            continue

        # A single line/group exceeds DISCORD_LIMIT on its own.
        raw_group = "\n".join(group).strip()
        if len(chunks) >= max_parts - 1:
            remainder = "\n".join(lines[idx:]).strip()
            chunks.append(truncate_stable_message(remainder, url))
            return [c for c in chunks if c]
        chunks.append(truncate_utf16(raw_group, DISCORD_LIMIT))
        idx = next_idx

    if current:
        text = joined(current)
        if text:
            chunks.append(text)
    return chunks or [truncate_stable_message(message, url)]


def format_messages(*, tag: str, name: str, body: str, url: str) -> list[str]:
    if (
        re.fullmatch(r"selfdev-linux-[0-9a-f]{40}", tag)
        or re.fullmatch(r"v1\.0\.0-preview\.[1-9][0-9]*", tag)
    ):
        return [format_message(tag=tag, name=name, body=body, url=url)]
    title = f"## {tag}" + (f" — {name}" if name and name != tag else "")
    notes = release_notes_for_discord(body)
    message = title if not notes else f"{title}\n{notes}"
    return split_stable_message(message, url)


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


def build_discord_multipart(
    *, payload: dict[str, Any], attachments: list[Path]
) -> tuple[bytes, str]:
    boundary = f"----jcode-discord-{uuid.uuid4().hex}"
    chunks: list[bytes] = []
    payload_bytes = json.dumps(payload).encode("utf-8")
    chunks.extend([
        f"--{boundary}\r\n".encode("ascii"),
        b'Content-Disposition: form-data; name="payload_json"\r\n',
        b"Content-Type: application/json\r\n\r\n",
        payload_bytes,
        b"\r\n",
    ])
    for index, file_path in enumerate(attachments):
        mime_type = mimetypes.guess_type(file_path.name)[0] or "application/octet-stream"
        filename = file_path.name.replace('"', "")
        chunks.extend([
            f"--{boundary}\r\n".encode("ascii"),
            (
                f'Content-Disposition: form-data; name="files[{index}]"; '
                f'filename="{filename}"\r\n'
            ).encode("utf-8"),
            f"Content-Type: {mime_type}\r\n\r\n".encode("ascii"),
            file_path.read_bytes(),
            b"\r\n",
        ])
    chunks.append(f"--{boundary}--\r\n".encode("ascii"))
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def post_to_discord(
    *, webhook_url: str, content: str, attachments: list[Path] | None = None
) -> dict[str, Any]:
    payload = {
        "content": content,
        # Release notes are generated from commit subjects. Never let
        # them turn @everyone or user-looking text into real mentions.
        "allowed_mentions": {"parse": []},
    }
    if attachments:
        data, content_type = build_discord_multipart(
            payload=payload, attachments=attachments
        )
    else:
        data = json.dumps(payload).encode("utf-8")
        content_type = "application/json"
    request = urllib.request.Request(
        discord_webhook_url(webhook_url),
        data=data,
        headers={
            "Content-Type": content_type,
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


def is_git_stale_announcement(discord_path: Path, changelog_path: Path) -> bool:
    """Return True if changelog_path was updated after discord_path in git."""
    repo_dir = changelog_path.parent
    try:
        discord_dirty = subprocess.run(
            ["git", "-C", str(repo_dir), "status", "--porcelain", "--", str(discord_path)],
            capture_output=True,
            text=True,
            check=False,
        )
        changelog_dirty = subprocess.run(
            ["git", "-C", str(repo_dir), "status", "--porcelain", "--", str(changelog_path)],
            capture_output=True,
            text=True,
            check=False,
        )
        if discord_dirty.returncode == 0 and discord_dirty.stdout.strip():
            return False
        if changelog_dirty.returncode == 0 and changelog_dirty.stdout.strip():
            return True
        discord_rev = subprocess.run(
            ["git", "-C", str(repo_dir), "log", "-1", "--format=%H", "--", str(discord_path)],
            capture_output=True,
            text=True,
            check=False,
        )
        changelog_rev = subprocess.run(
            ["git", "-C", str(repo_dir), "log", "-1", "--format=%H", "--", str(changelog_path)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return False
    if discord_rev.returncode != 0 or changelog_rev.returncode != 0:
        return False
    d_sha = discord_rev.stdout.strip()
    c_sha = changelog_rev.stdout.strip()
    if not (re.fullmatch(r"[0-9a-f]{40}", d_sha) and re.fullmatch(r"[0-9a-f]{40}", c_sha)):
        return False
    if d_sha == c_sha:
        return False
    ancestor = subprocess.run(
        ["git", "-C", str(repo_dir), "merge-base", "--is-ancestor", d_sha, c_sha],
        capture_output=True,
        check=False,
    )
    return ancestor.returncode == 0


def _resolve_attachment_paths(raw_list: Any) -> list[Path]:
    if raw_list is None:
        return []
    if not isinstance(raw_list, list):
        raise ValueError("attachments must be an array of relative file paths")
    base = ANNOUNCEMENT_DIR.resolve()
    resolved: list[Path] = []
    for item in raw_list:
        if not isinstance(item, str) or not item.strip():
            raise ValueError("each attachment must be a nonempty relative path string")
        candidate = (base / item).resolve()
        try:
            candidate.relative_to(base)
        except ValueError as error:
            raise ValueError(f"attachment {item!r} escapes {ANNOUNCEMENT_DIR}") from error
        if not candidate.is_file():
            raise ValueError(f"attachment {item!r} does not exist")
        resolved.append(candidate)
    return resolved


def load_announcement_parts(tag: str) -> list[tuple[str, list[Path]]] | None:
    # Tags must never escape the checked-in announcement directory.
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", tag):
        return None
    path = ANNOUNCEMENT_DIR / f"{tag}.json"
    if not path.exists():
        return None
    expected_changelog_sha: str | None = None
    top_attachments: list[Path] = []
    parts: list[tuple[str, list[Path]]] = []
    try:
        parsed = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(parsed, dict):
            messages = parsed.get("messages")
            sha_value = parsed.get("changelog_sha256")
            if sha_value is not None:
                if not isinstance(sha_value, str) or not re.fullmatch(r"[0-9a-f]{64}", sha_value):
                    raise ValueError("changelog_sha256 must be a 64-char lowercase hex sha256")
                expected_changelog_sha = sha_value
            top_attachments = _resolve_attachment_paths(parsed.get("attachments"))
        else:
            messages = parsed
        if not isinstance(messages, list) or not messages:
            raise ValueError("expected a nonempty array of content strings")
        for index, entry in enumerate(messages):
            if isinstance(entry, str):
                content = entry
                entry_attachments: list[Path] = []
            elif isinstance(entry, dict):
                content = entry.get("content")
                entry_attachments = _resolve_attachment_paths(entry.get("attachments"))
            else:
                raise ValueError(f"message {index + 1} must be a string or object")
            if not isinstance(content, str) or not content.strip():
                raise ValueError(f"message {index + 1} must be a nonempty string")
            if utf16_len(content) > DISCORD_LIMIT:
                raise ValueError(f"message {index + 1} exceeds 2000 UTF-16 units")
            combined_attachments = (top_attachments if index == 0 else []) + entry_attachments
            parts.append((content, combined_attachments))
    except (OSError, ValueError, UnicodeError) as error:
        raise RuntimeError(f"Invalid Discord announcement {path}: {error}") from error

    changelog_path = ANNOUNCEMENT_DIR.parent / f"{tag}.json"
    if changelog_path.exists():
        if expected_changelog_sha is not None:
            actual_sha = hashlib.sha256(changelog_path.read_bytes()).hexdigest()
            if actual_sha != expected_changelog_sha:
                print(
                    f"::warning::Ignoring stale Discord announcement {path}: "
                    f"changelog_sha256 does not match {changelog_path}; "
                    "falling back to GitHub release notes.",
                    file=sys.stderr,
                )
                return None
        elif is_git_stale_announcement(path, changelog_path):
            print(
                f"::warning::Ignoring stale Discord announcement {path}: "
                f"{changelog_path} was updated in a newer commit; "
                "falling back to GitHub release notes.",
                file=sys.stderr,
            )
            return None
    return parts


def load_announcement(tag: str) -> list[str] | None:
    parts = load_announcement_parts(tag)
    return None if parts is None else [content for content, _ in parts]


def multipart_marker(tag: str, digest: str, index: int, state: str) -> str:
    return f"<!-- jcode-discord-part:{tag}:{digest}:{index}:{state} -->"


def announce_multipart(
    *,
    repository: str,
    tag: str,
    token: str,
    webhook_url: str,
    messages: list[str],
    attachments_per_part: list[list[Path]] | None = None,
) -> str | None:
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
        part_attachments = (
            attachments_per_part[index - 1]
            if attachments_per_part and index - 1 < len(attachments_per_part)
            else None
        )
        try:
            message = post_to_discord(
                webhook_url=webhook_url,
                content=content,
                attachments=part_attachments or None,
            )
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


SELFDEV_IMAGE_EXTENSIONS = {".png", ".jpg", ".jpeg", ".webp"}
MAX_SELFDEV_ATTACHMENTS = 4


def discover_selfdev_attachments(
    *, tag: str, body: str, repo_root: Path | None = None
) -> list[Path]:
    """Return image files added/modified under changelog/discord/ in a selfdev release range."""
    match = re.fullmatch(r"selfdev-linux-([0-9a-f]{40})", tag)
    if not match:
        return []
    head_sha = match.group(1)
    root = repo_root or Path(__file__).resolve().parent.parent

    compare_match = re.search(
        r"/compare/([0-9a-f]{7,40})\.\.\.([0-9a-f]{7,40})",
        body or "",
    )
    if compare_match:
        rev_range = f"{compare_match.group(1)}..{compare_match.group(2)}"
    else:
        rev_range = f"{head_sha}~1..{head_sha}"

    try:
        proc = subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "diff",
                "--name-only",
                "--diff-filter=AM",
                rev_range,
                "--",
                "changelog/discord",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return []
    if proc.returncode != 0:
        return []

    discord_root = (root / "changelog" / "discord").resolve()
    attachments: list[Path] = []
    for rel in sorted(line.strip() for line in proc.stdout.splitlines() if line.strip()):
        candidate = (root / rel).resolve()
        if candidate.suffix.lower() not in SELFDEV_IMAGE_EXTENSIONS:
            continue
        try:
            candidate.relative_to(discord_root)
        except ValueError:
            continue
        if candidate.is_file():
            attachments.append(candidate)
            if len(attachments) >= MAX_SELFDEV_ATTACHMENTS:
                break
    return attachments


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
        parts = load_announcement_parts(tag)
        attachments_per_part = (
            [atts for _, atts in parts]
            if parts is not None and [content for content, _ in parts] == messages
            else None
        )
        return announce_multipart(
            repository=repository,
            tag=tag,
            token=token,
            webhook_url=webhook_url,
            messages=messages,
            attachments_per_part=attachments_per_part,
        )

    formatted_messages = format_messages(
        tag=tag,
        name=release.get("name") or tag,
        body=body,
        url=release["html_url"],
    )
    selfdev_attachments = discover_selfdev_attachments(tag=tag, body=body)
    if len(formatted_messages) > 1:
        attachments_per_part = (
            [selfdev_attachments] + [[] for _ in formatted_messages[1:]]
            if selfdev_attachments
            else None
        )
        return announce_multipart(
            repository=repository,
            tag=tag,
            token=token,
            webhook_url=webhook_url,
            messages=formatted_messages,
            attachments_per_part=attachments_per_part,
        )

    content = formatted_messages[0]
    message = post_to_discord(
        webhook_url=webhook_url,
        content=content,
        attachments=selfdev_attachments or None,
    )
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
