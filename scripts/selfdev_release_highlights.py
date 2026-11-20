"""Generate public self-dev release highlights from explicit notes or public git subjects."""
import html
import os
import re
import subprocess
import sys
from pathlib import Path

TAG = re.compile(r'selfdev-linux-([0-9a-f]{40})')
MAX_ITEMS = 8


def bullets(text):
    items = []
    for line in text[:16384].splitlines():
        plain = ''.join(c if c.isprintable() else ' ' for c in line)
        plain = re.sub(r'^\s*[-*•]\s+', '', plain)
        plain = ' '.join(plain.split())
        if not plain:
            continue
        if len(plain) > 200:
            plain = plain[:199].rstrip() + '…'
        # Treat each entry as text, not authored markup or an active mention.
        plain = html.escape(plain, quote=False).replace('@', '＠')
        plain = re.sub(r'([\\`*_\[\]~|])', r'\\\1', plain)
        plain = re.sub(r'^([#+=-]|\d+[.)])(?=\s|$)', lambda m: m[0][:-1] + '\\' + m[0][-1], plain)
        items.append('- ' + plain)
        if len(items) == MAX_ITEMS:
            break
    return items


def git(*args):
    return subprocess.check_output(['git', *args], encoding='utf-8', errors='replace').strip()


def body(sha, notes, tags):
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('invalid commit SHA')
    items = bullets(notes)
    authored = bool(items)
    previous = None
    for tag in tags:  # API release order, newest first; ignore unrelated histories.
        match = TAG.fullmatch(tag.strip())
        if match and (base := match[1]) != sha:
            status = subprocess.run(['git', 'merge-base', '--is-ancestor', base, sha]).returncode
            if status == 0:
                previous = base
                break
            if status != 1:
                raise ValueError('unable to inspect release ancestry')
    if not items:
        revision = f'{previous}..{sha}' if previous else sha
        items = bullets(git('log', '--format=%s', '-n', str(MAX_ITEMS if previous else 1), revision))
    result = f'Portable Linux x86_64 self-dev build of {sha}.\n\n## Highlights\n\n' + '\n'.join(items or ['- Build ' + sha[:12]]) + '\n'
    if previous:
        revision = f'{previous}..{sha}'
        count = int(git('rev-list', '--count', revision))
        compare = f'https://github.com/1jehuang/jcode/compare/{previous}...{sha}'
        result += f'\n## Bundled changes\n\n[View all {count} commits]({compare})\n'
        if authored:
            result += '\n'.join(bullets(git('log', '--format=%s', '-n', str(MAX_ITEMS), revision))) + '\n'
    return result


if __name__ == '__main__':
    with Path(sys.argv[1]).open() as tags:
        sys.stdout.write(body(os.environ['SHA'], os.environ.get('RELEASE_NOTES', ''), tags))
