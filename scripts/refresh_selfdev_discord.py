"""Edit a previously announced release in place. Never create a Discord message."""
import json
import os
import re
import sys
import urllib.parse
import urllib.request

import post_discord_release as announcements

SELFDEV_TAG_RE = re.compile(r'selfdev-linux-[0-9a-f]{40}')
STABLE_TAG_RE = re.compile(r'v\d+\.\d+\.\d+')


def _matches_existing_message(*, tag, url, content, is_selfdev):
    links = (url, f'<{url}>', f'[Download & release notes](<{url}>)')
    link_pattern = r'(?<!\S)(?:' + '|'.join(map(re.escape, links)) + r')(?!\S)'
    if re.search(link_pattern, content):
        return True
    if not is_selfdev:
        heading_pattern = (
            r'^(?:#\s+jcode\s+|##\s+)'
            + re.escape(tag)
            + r'(?:$|[\s:—–-])'
        )
        if re.search(heading_pattern, content.lstrip()):
            return True
    return False


def refresh(*, tag, message_id, webhook_url, repository, token):
    is_selfdev = bool(SELFDEV_TAG_RE.fullmatch(tag))
    is_stable = bool(STABLE_TAG_RE.fullmatch(tag))
    if not (is_selfdev or is_stable):
        raise ValueError('Expected a self-dev or stable release tag')
    if not re.fullmatch(r'[1-9][0-9]{16,19}', message_id):
        raise ValueError('Invalid Discord message ID')
    release = announcements.fetch_release(repository=repository, tag=tag, token=token)
    url = f'https://github.com/{repository}/releases/tag/{tag}'
    body = release.get('body') or ''
    expected_prerelease = is_selfdev
    if (release.get('tag_name') != tag or release.get('html_url') != url
            or release.get('draft') is not False
            or release.get('prerelease') is not expected_prerelease
            or announcements.announcement_marker(tag) not in body.splitlines()):
        raise ValueError('Expected an announced public release matching tag')

    parts = urllib.parse.urlsplit(webhook_url)
    path = parts.path.rstrip('/') + '/messages/' + message_id
    endpoint = parts._replace(path=path, fragment='').geturl()
    def request(method, payload=None):
        data = None if payload is None else json.dumps(payload).encode()
        req = urllib.request.Request(endpoint, data=data, method=method, headers={
            'Content-Type': 'application/json', 'User-Agent': 'jcode-release-bot/2.0'})
        with urllib.request.urlopen(req, timeout=30) as response:
            message = json.load(response)
        if message.get('id') != message_id:
            raise ValueError('Discord returned an unexpected message ID')
        return message

    previous = request('GET')
    if not _matches_existing_message(
        tag=tag,
        url=url,
        content=previous.get('content') or '',
        is_selfdev=is_selfdev,
    ):
        raise ValueError('Existing message does not link to the exact release')

    content = None
    if is_stable:
        override = announcements.load_announcement(tag)
        if override is not None:
            if len(override) != 1:
                raise ValueError('Cannot refresh a single Discord message with a multi-part override')
            content = override[0]
    if content is None:
        content = announcements.format_message(tag=tag, name=release.get('name') or tag,
                                               body=body, url=url)
    request('PATCH', {'content': content, 'flags': 4, 'allowed_mentions': {'parse': []}})


if __name__ == '__main__':
    try:
        refresh(tag=os.environ['TAG'], message_id=os.environ['MESSAGE_ID'],
                webhook_url=os.environ['DISCORD_RELEASE_WEBHOOK'],
                repository=os.environ['GITHUB_REPOSITORY'], token=os.environ['GH_TOKEN'])
    except Exception as exc:
        # Transport exceptions may contain the secret webhook URL.
        print(f'Refresh failed ({type(exc).__name__}). No new message was posted.', file=sys.stderr)
        sys.exit(1)
