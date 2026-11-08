# Read-only remote projection. No sidecars or canonical session writes.
import collections
import json
import mmap
import os
from pathlib import Path
import re
import socket
import sys
import time


def timing(stage, outcome, started):
    # Fixed labels only. Never print exceptions, identifiers or projected data.
    print('[CLOUD_TIMING] stage={} outcome={} duration_ms={}'.format(
        stage, outcome, int((time.monotonic() - started) * 1000)), file=sys.stderr)


def timed(stage):
    def decorate(function):
        def run(*args, **kwargs):
            started = time.monotonic()
            outcome = 'error'
            try:
                result = function(*args, **kwargs)
                outcome = 'success'
                return result
            finally:
                timing(stage, outcome, started)
        return run
    return decorate

# Consume complete strings in C, including escaped quotes and backslashes.
STRING = re.compile(br'"[^"\\]*(?:\\.[^"\\]*)*"', re.DOTALL)
# Coalesce adjacent strings, keys, numbers and punctuation into one C match.
# Only container delimiters and malformed quotes need a Python iteration.
STRUCT = re.compile(br'(?P<skip>(?:"[^"\\]*(?:\\.[^"\\]*)*"|[^"{}\[\]]+){1,64})|(?P<brace>[{}\[\]])|(?P<bad>")', re.DOTALL)
SPACE = b' \r\n\t'
BULKY = {'env_snapshots', 'memory_injections', 'replay_events', 'compaction', 'system_prompt'}
MAX_MESSAGES = 80
MAX_TEXT = 8192
MAX_OUTPUT = 2 * 1024 * 1024
META_FIELDS = set('parent_id title custom_title updated_at provider_session_id model_usage_turn_id provider_key route_api_method model reasoning_effort context_cap subagent_model agent_model_overrides improve_mode autoreview_enabled autojudge_enabled is_canary testing_build working_dir original_working_dir short_name status last_pid last_active_at is_debug saved save_label'.split())
INVENTORY_FIELDS = {'id', 'migration_epoch', 'working_dir', 'custom_title', 'title', 'updated_at'}


def string_end(data, start):
    match = STRING.match(data, start)
    if match is None:
        raise ValueError('unterminated string')
    return match.end()


def skip_space(data, pos):
    while pos < len(data) and data[pos] in SPACE:
        pos += 1
    return pos


def value_end(data, pos, ranges=None):
    if data[pos] == 34:
        return string_end(data, pos)
    if data[pos] not in (123, 91):
        end = pos
        while end < len(data) and data[end] not in b',}]\r\n \t':
            end += 1
        return end
    stack = [125 if data[pos] == 123 else 93]
    for match in STRUCT.finditer(data, pos + 1):
        kind = match.lastgroup
        if kind == 'skip':
            continue
        if kind == 'bad':
            raise ValueError('unterminated string')
        char = data[match.start()]
        if char in (123, 91):
            if ranges is not None and len(stack) == 1:
                begin = match.start()
            stack.append(125 if char == 123 else 93)
        elif char != stack.pop():
            raise ValueError('mismatched container')
        elif not stack:
            return match.end()
        elif ranges is not None and len(stack) == 1:
            ranges.append((begin, match.end()))
    raise ValueError('unterminated container')


def fields(data, pos=0, message_ranges=None):
    pos = skip_space(data, pos)
    if data[pos] != 123:
        raise ValueError('expected object')
    pos += 1
    while True:
        pos = skip_space(data, pos)
        if data[pos] == 125:
            return pos + 1
        end = string_end(data, pos)
        key = json.loads(data[pos:end])
        pos = skip_space(data, end)
        if data[pos] != 58:
            raise ValueError('expected colon')
        pos = skip_space(data, pos + 1)
        ranges = message_ranges if key in ('messages', 'append_messages') else None
        if ranges is not None:
            ranges.clear()
            if data[pos] != 91:
                raise ValueError('expected message array')
        end = value_end(data, pos, ranges)
        yield key, pos, end
        pos = skip_space(data, end)
        if data[pos] == 125:
            return pos + 1
        if data[pos] != 44:
            raise ValueError('expected comma')
        pos += 1


def journal_fields(data):
    # fields already scans every value. Its return value is the object boundary,
    # so journal entries need no redundant whole-container value_end scan.
    pos = 0
    while pos < len(data):
        pos = skip_space(data, pos)
        if pos == len(data):
            break
        pos = yield from fields(data, pos)


def recent_messages(data, start, end, messages, ranges=None):
    # Locate the tail without deserializing old message bodies. The range deque
    # is bounded even for snapshots with millions of messages.
    if ranges is None:
        ranges = collections.deque(maxlen=MAX_MESSAGES)
        pos = skip_space(data, start + 1)
        while pos < end and data[pos] != 93:
            stop = value_end(data, pos)
            ranges.append((pos, stop))
            pos = skip_space(data, stop)
            if data[pos] == 44:
                pos = skip_space(data, pos + 1)
    for pos, stop in ranges:
        # Oversized individual messages are projected without decoding their
        # potentially enormous tool payloads. Recent ordinary text is retained.
        if stop - pos <= 16 * 1024:
            messages.append(json.loads(data[pos:stop]))
            continue
        message = {}
        for key, begin, finish in fields(data, pos):
            if key in ('id', 'role') and finish - begin < 4096:
                message[key] = json.loads(data[begin:finish])
            elif key == 'content':
                text = []
                block = skip_space(data, begin + 1)
                while block < finish and data[block] != 93:
                    block_end = value_end(data, block)
                    for name, a, b in fields(data, block):
                        if name == 'text' and sum(map(len, text)) < MAX_TEXT:
                            # Decode only bounded strings, otherwise use a valid
                            # preview marker instead of slicing JSON mid-string.
                            text.append(json.loads(data[a:b]) if b - a <= MAX_TEXT else '[large text omitted from preview]')
                    block = skip_space(data, block_end)
                    if data[block] == 44:
                        block = skip_space(data, block + 1)
                message['content'] = [{'type': 'text', 'text': '\n'.join(text)[:MAX_TEXT]}]
        messages.append(message)


@timed('projection')
def project(path, preview):
    session = {}
    messages = collections.deque(maxlen=MAX_MESSAGES)
    with path.open('rb') as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        ranges = collections.deque(maxlen=MAX_MESSAGES) if preview else None
        for key, start, end in fields(data, message_ranges=ranges):
            if key == 'messages':
                if preview:
                    recent_messages(data, start, end, messages, ranges)
            elif key not in BULKY and (preview or key in INVENTORY_FIELDS) and end - start <= 256 * 1024:
                session[key] = json.loads(data[start:end])
    journal = path.with_suffix('.journal.jsonl')
    if journal.exists() and journal.stat().st_size:
        with journal.open('rb') as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
            for key, start, stop in journal_fields(data):
                if key == 'meta':
                    meta = {}
                    route_present = False
                    for name, a, b in fields(data, start):
                        if name == 'route_api_method_present':
                            route_present = data[a:b] == b'true'
                        if name in META_FIELDS and (preview or name in INVENTORY_FIELDS) and b - a <= 256 * 1024:
                            meta[name] = json.loads(data[a:b])
                    if not route_present:
                        meta.pop('route_api_method', None)
                    session.update(meta)
                elif key == 'append_messages' and preview:
                    recent_messages(data, start, stop, messages)
    if preview:
        session['messages'] = list(messages)
        for key in ('env_snapshots', 'memory_injections', 'replay_events'):
            session[key] = []
    return session


@timed('remote_request')
def main():
    mode, ids = sys.argv[1:3]
    base = Path(os.environ.get('JCODE_HOME', str(Path.home() / '.jcode')))
    result = []
    for sid in ids.splitlines():
        started = time.monotonic()
        if not sid or not all(c.isascii() and (c.isalnum() or c in '_-') for c in sid):
            timing('inventory_entry', 'skipped_invalid_target', started)
            if mode == 'preview':
                raise ValueError('invalid session id')
            continue
        try:
            session = project(base / 'sessions' / (sid + '.json'), mode == 'preview')
            if session.get('id') != sid:
                raise ValueError('session identity mismatch')
            if mode == 'preview':
                encoded = json.dumps(session, separators=(',', ':'), ensure_ascii=False).encode()
                while len(encoded) > MAX_OUTPUT and session['messages']:
                    session['messages'].pop(0)
                    encoded = json.dumps(session, separators=(',', ':'), ensure_ascii=False).encode()
                if len(encoded) > MAX_OUTPUT:
                    raise ValueError('preview metadata too large')
                sys.stdout.buffer.write(encoded + b'\0\0')
                state = Path(os.environ['JCODE_RUNTIME_DIR']) / 'durable-state' if os.environ.get('JCODE_RUNTIME_DIR') else base / 'state'
                telemetry = state / 'session-telemetry' / (sid + '.json')
                if telemetry.exists() and telemetry.stat().st_size < 65536:
                    sys.stdout.buffer.write(telemetry.read_bytes())
                return
            lease_path = base / 'session_leases' / (sid + '.json')
            lease = json.loads(lease_path.read_bytes()) if lease_path.exists() else {}
            if lease.get('away_host'):
                timing('inventory_entry', 'skipped_away', started)
                continue
            result.append({'session_id': sid, 'epoch': max(session.get('migration_epoch', 0), lease.get('epoch', 0)), 'cwd': session.get('working_dir'), 'title': session.get('custom_title') if session.get('custom_title') is not None else session.get('title'), 'updated_at': session.get('updated_at')})
            timing('inventory_entry', 'success', started)
        except (OSError, ValueError, TypeError, IndexError):
            timing('inventory_entry' if mode != 'preview' else 'remote_preview', 'error', started)
            if mode == 'preview':
                raise
    print(json.dumps({'host': socket.gethostname(), 'sessions': result}))


if __name__ == '__main__':
    try:
        main()
    except Exception:
        # Tracebacks include paths and may quote private input. The stage timer
        # already records failure, and the exit status preserves the contract.
        sys.exit(1)
