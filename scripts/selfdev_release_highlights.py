"""Generate public self-dev release highlights from explicit notes or public git subjects."""
import html
import os
import re
import subprocess
import sys
from pathlib import Path

TAG = re.compile(r'selfdev-linux-([0-9a-f]{40})')
SEMVER_TAG = re.compile(r'v[0-9]+\.[0-9]+\.[0-9]+')
MAX_ITEMS = 8
MAX_ITEM_CHARS = 360
MAX_BUNDLED_ITEMS = 50
NO_USER_VISIBLE_RE = re.compile(r'^no user-visible changes\.?$', re.I)


def truncate_plain(text, limit=MAX_ITEM_CHARS):
    if len(text) <= limit:
        return text
    prefix = text[:limit - 1]
    # Prefer a word boundary in the second half so words and tokens are not cut in half.
    if ' ' in prefix:
        last_space = prefix.rfind(' ')
        if last_space >= limit // 2:
            prefix = prefix[:last_space]
    # Never leave an unclosed inline-code backtick span.
    if prefix.count('`') % 2 == 1:
        last_tick = prefix.rfind('`')
        if last_tick > 0:
            prefix = prefix[:last_tick]
    prefix = prefix.rstrip(' ,;:([{-')
    if not prefix:
        prefix = text[:limit - 1].rstrip()
    return prefix + '…'


def bullets(text, limit=MAX_ITEMS):
    items = []
    for line in text[:65536].splitlines():
        plain = ''.join(c if c.isprintable() else ' ' for c in line)
        plain = re.sub(r'^\s*[-*•]\s+', '', plain)
        plain = ' '.join(plain.split())
        if not plain:
            continue
        plain = truncate_plain(plain, MAX_ITEM_CHARS)
        # Treat each entry as text, not authored markup or an active mention.
        plain = html.escape(plain, quote=False).replace('@', '＠')
        plain = re.sub(r'([\\`*_\[\]~|])', r'\\\1', plain)
        plain = re.sub(r'^([#+=-]|\d+[.)])(?=\s|$)', lambda m: m[0][:-1] + '\\' + m[0][-1], plain)
        items.append('- ' + plain)
        if len(items) == limit:
            break
    return items


def is_no_changes_placeholder(items):
    return bool(items) and all(
        NO_USER_VISIBLE_RE.fullmatch(item.removeprefix('- ').strip())
        for item in items
    )


def git(*args):
    return subprocess.check_output(['git', *args], encoding='utf-8', errors='replace').strip()


def body(sha, notes, tags):
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('invalid commit SHA')
    items = bullets(notes)
    previous = None
    for raw_tag in tags:  # API release order, newest first; ignore unrelated histories.
        tag = raw_tag.strip()
        base = None
        if match := TAG.fullmatch(tag):
            base = match[1]
        elif SEMVER_TAG.fullmatch(tag):
            resolved = subprocess.run(
                ['git', 'rev-parse', '--verify', '--quiet', f'refs/tags/{tag}^{{commit}}'],
                capture_output=True,
                text=True,
            )
            if resolved.returncode == 0:
                base = resolved.stdout.strip()
        if base and base != sha:
            status = subprocess.run(['git', 'merge-base', '--is-ancestor', base, sha]).returncode
            if status == 0:
                previous = base
                break
            if status != 1:
                raise ValueError('unable to inspect release ancestry')
    count = int(git('rev-list', '--count', f'{previous}..{sha}')) if previous else 0
    # A single agent's "No user-visible changes." placeholder must not hide
    # earlier commits bundled into the same release.
    if previous and count > 1 and is_no_changes_placeholder(items):
        items = []
    authored = bool(items)
    if not items:
        revision = f'{previous}..{sha}' if previous else sha
        items = bullets(git('log', '--format=%s', '-n', str(MAX_ITEMS if previous else 1), revision))
    result = f'Portable Linux x86_64 self-dev build of {sha}.\n\n## Highlights\n\n' + '\n'.join(items or ['- Build ' + sha[:12]]) + '\n'
    if previous:
        revision = f'{previous}..{sha}'
        compare = f'https://github.com/1jehuang/jcode/compare/{previous}...{sha}'
        commit_noun = 'commit' if count == 1 else 'commits'
        result += f'\n## Bundled changes\n\n[View all {count} {commit_noun}]({compare})\n'
        if authored:
            bundled = bullets(
                git('log', '--format=%s', '-n', str(MAX_BUNDLED_ITEMS), revision),
                limit=MAX_BUNDLED_ITEMS,
            )
            if bundled:
                result += '\n'.join(bundled) + '\n'
            if count > len(bundled):
                remaining = count - len(bundled)
                more_noun = 'commit' if remaining == 1 else 'commits'
                result += f'- … and {remaining} more {more_noun} in the comparison above.\n'
        elif count > len(items):
            remaining_limit = max(0, MAX_BUNDLED_ITEMS - len(items))
            extra = (
                bullets(
                    git('log', '--format=%s', f'--skip={len(items)}', '-n', str(remaining_limit), revision),
                    limit=remaining_limit,
                )
                if remaining_limit > 0
                else []
            )
            if extra:
                result += '\n'.join(extra) + '\n'
            shown = len(items) + len(extra)
            if count > shown:
                remaining = count - shown
                more_noun = 'commit' if remaining == 1 else 'commits'
                result += f'- … and {remaining} more {more_noun} in the comparison above.\n'
    return result


if __name__ == '__main__':
    with Path(sys.argv[1]).open() as tags:
        sys.stdout.write(body(os.environ['SHA'], os.environ.get('RELEASE_NOTES', ''), tags))
