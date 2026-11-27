"""Read-only VM workspace inventory. Invoked by the bounded SSH telemetry worker.

Presence markers cover whole turns (including tools) in both agent turn loops.
Read the per-session registry, not process counts, so shared daemons and swarm
workers count correctly. No subprocesses, recursive filesystem walks or writes.
"""
import json
import mmap
import re
import os
import pathlib
import sys
import time

MAX_ITEMS = 4096
MAX_JSON_BYTES = 32 * 1024 * 1024
FIELDS = {'working_dir', 'is_debug', 'is_canary'}
# Validate skipped strings, including UTF-8, without materializing their text.
UTF8 = (rb'[\xc2-\xdf][\x80-\xbf]|\xe0[\xa0-\xbf][\x80-\xbf]|'
        rb'[\xe1-\xec\xee-\xef][\x80-\xbf]{2}|\xed[\x80-\x9f][\x80-\xbf]|'
        rb'\xf0[\x90-\xbf][\x80-\xbf]{2}|[\xf1-\xf3][\x80-\xbf]{3}|'
        rb'\xf4[\x80-\x8f][\x80-\xbf]{2}')
ASCII = rb'[\x20-\x21\x23-\x5b\x5d-\x7f]*'
STRING = rb'"' + ASCII + rb'(?:(?:\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4})|' + UTF8 + rb')' + ASCII + rb')*"'
WS = rb'[ \t\r\n]*'
SCALAR = STRING + rb'|true|false|null|-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?'


def value_pattern(depth):
    scalar = rb'(?:' + SCALAR + rb')'
    if not depth:
        return scalar
    child = value_pattern(depth - 1)
    pair = STRING + WS + b':' + WS + child
    return (rb'(?:' + scalar + rb'|\[' + WS + rb'(?:' + child + WS +
            rb'(?:,' + WS + child + WS + rb')*)?\]|\{' + WS + rb'(?:' + pair +
            WS + rb'(?:,' + WS + pair + WS + rb')*)?\})')


# A shallow complete value (typically one transcript message) is validated in C.
# Limit each match's window so an arbitrarily large vector cannot monopolize the
# deadline or allocate regex backtracking state proportional to the transcript.
VALUE = re.compile(value_pattern(3))
ASCII_RUN = re.compile(ASCII)
ESCAPE = re.compile(rb'\\(?:["\\/bfnrt]|u[0-9a-fA-F]{4})')
UTF8_TOKEN = re.compile(UTF8)
SPACE = re.compile(WS)
SCAN_WINDOW = 64 * 1024


def metadata(data, check_time, journal=False):
    size = len(data)

    def space(pos):
        while True:
            limit = min(size, pos + SCAN_WINDOW)
            pos = SPACE.match(data, pos, limit).end()
            if pos < limit or pos == size:
                return pos
            check_time()

    def string(pos):
        if data[pos:pos + 1] != b'"':
            raise ValueError('invalid JSON string')
        pos += 1
        while True:
            check_time()
            limit = min(size, pos + SCAN_WINDOW)
            pos = ASCII_RUN.match(data, pos, limit).end()
            if pos == limit and pos < size:
                continue
            first = data[pos:pos + 1]
            if first == b'"':
                return pos + 1
            token = (ESCAPE if first == b'\\' else UTF8_TOKEN).match(data, pos)
            if token is None:
                raise ValueError('invalid JSON string')
            pos = token.end()

    def skip(pos, depth=0):
        check_time()
        if depth > 128:
            raise ValueError('session nesting limit')
        token = VALUE.match(data, pos, min(size, pos + SCAN_WINDOW))
        if token is not None:
            return token.end()
        first = data[pos:pos + 1]
        if first == b'"':
            end = string(pos)
            check_time()
            return end
        if first not in (b'{', b'['):
            raise ValueError('invalid JSON value')
        closing = b'}' if first == b'{' else b']'
        pos = space(pos + 1)
        if data[pos:pos + 1] == closing:
            return pos + 1
        while True:
            if first == b'{':
                pos = space(string(pos))
                if data[pos:pos + 1] != b':':
                    raise ValueError('missing JSON colon')
                pos = space(pos + 1)
            pos = space(skip(pos, depth + 1))
            if data[pos:pos + 1] == closing:
                return pos + 1
            if data[pos:pos + 1] != b',':
                raise ValueError('missing JSON separator')
            pos = space(pos + 1)

    def obj(pos, wanted):
        if data[pos:pos + 1] != b'{':
            raise ValueError('session must be an object')
        result = {}
        pos = space(pos + 1)
        if data[pos:pos + 1] == b'}':
            return result, pos + 1
        while True:
            check_time()
            end = string(pos)
            # Unrecognized long keys need validation, not decoding/allocation.
            key = json.loads(data[pos:end]) if end - pos <= 1024 else None
            pos = space(end)
            if data[pos:pos + 1] != b':':
                raise ValueError('missing JSON colon')
            pos = space(pos + 1)
            if journal and wanted == {'meta'} and key == 'meta' and data[pos:pos + 1] == b'{':
                result[key], pos = obj(pos, FIELDS)
            else:
                end = skip(pos)
                if key in wanted:
                    if journal and wanted == {'meta'}:
                        # Only object-valued meta participates in journal replay.
                        result[key] = None
                    else:
                        if not journal and end - pos > 16384:
                            raise ValueError('oversized session field')
                        # Containers must replace a prior duplicate scalar,
                        # matching JSON's last-key-wins semantics.
                        result[key] = json.loads(data[pos:end])
                pos = end
            pos = space(pos)
            if data[pos:pos + 1] == b'}':
                return result, pos + 1
            if data[pos:pos + 1] != b',':
                raise ValueError('missing JSON separator')
            pos = space(pos + 1)

    check_time()
    result, end = obj(space(0), {'meta'} if journal else FIELDS)
    if space(end) != size:
        raise ValueError('trailing session data')
    check_time()
    return result


def session_fields(path, check_time):
    # Large sessions journal metadata without rewriting the snapshot. The newest
    # complete record is authoritative, exactly as load_startup_stub handles it.
    journal = path.with_suffix('.journal.jsonl')
    try:
        with open(journal, 'rb') as stream:
            if os.fstat(stream.fileno()).st_size:
                with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
                    # Check every record's byte bound, including superseded ones.
                    # Only the newest valid metadata record needs JSON parsing.
                    start = cursor = 0
                    while cursor < len(data):
                        check_time()
                        limit = min(len(data), cursor + SCAN_WINDOW)
                        newline = data.find(b'\n', cursor, limit)
                        end = newline + 1 if newline >= 0 else limit
                        if end - start > MAX_JSON_BYTES:
                            raise ValueError('oversized journal record')
                        if newline >= 0:
                            start = end
                        cursor = end
                    end = len(data)
                    while end:
                        check_time()
                        # Include the newline in the same bound as readline.
                        start = data.rfind(b'\n', 0, end - 1) + 1
                        try:
                            entry = metadata(data[start:end], check_time, journal=True)
                            meta = entry.get('meta')
                            if isinstance(meta, dict):
                                return {key: meta.get(key) for key in FIELDS}
                        except (ValueError, AttributeError):
                            pass  # tolerate partial/malformed appends, like replay
                        end = start
    except FileNotFoundError:
        pass

    with open(path, 'rb') as stream:
        if os.fstat(stream.fileno()).st_size == 0:
            raise ValueError('empty session snapshot')
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            return metadata(data, check_time)


def collect(current, home=None, state=None, alive=None, current_session_id=None):
    home = pathlib.Path(home or pathlib.Path.home()).resolve()
    state = pathlib.Path(state or os.environ.get("JCODE_HOME", str(home / ".jcode")))
    deadline = time.monotonic() + 4
    truncated = False

    def check_time():
        if time.monotonic() > deadline:
            raise TimeoutError("workspace inventory deadline")

    def children(path):
        nonlocal truncated
        check_time()
        try:
            with os.scandir(path) as iterator:
                entries = []
                for entry in iterator:
                    check_time()
                    if len(entries) >= MAX_ITEMS:
                        truncated = True
                        break
                    entries.append(pathlib.Path(entry.path))
                return entries
        except FileNotFoundError:
            return []

    def read_json(path):
        check_time()
        with open(path, "rb") as stream:
            raw = stream.read(MAX_JSON_BYTES + 1)
        if len(raw) > MAX_JSON_BYTES:
            raise ValueError("oversized session metadata")
        return json.loads(raw)

    def process_alive(pid):
        try:
            os.kill(pid, 0)
            # A zombie has exited, even though kill(0) succeeds.
            try:
                stat = pathlib.Path('/proc') / str(pid) / 'stat'
                if stat.read_text().rsplit(')', 1)[1].strip().startswith('Z'):
                    return False
            except FileNotFoundError:
                return False
            return True
        except ProcessLookupError:
            return False
        except PermissionError:
            return True

    alive = alive or process_alive

    def marker_pid(path):
        try:
            raw = path.read_text().strip()
            pid = int(raw)
            return pid if pid > 0 and alive(pid) else None
        except (FileNotFoundError, ValueError):
            return None

    def directory(raw):
        if not isinstance(raw, str) or not raw or not os.path.isabs(raw):
            return None
        path = pathlib.Path(raw).resolve()
        return path if path.is_dir() else None

    def repo_root(path):
        for candidate in (path, *path.parents):
            if (candidate / '.git').exists():
                return candidate
        return path

    roots = set()
    leases = {}
    for path in children(state / 'session_leases'):
        if path.suffix != '.json':
            continue
        try:
            lease = read_json(path)
        except FileNotFoundError:
            continue
        root = directory(lease.get('repo_root'))
        leases[path.stem] = (lease, root)
        if root:
            roots.add(root)

    current_path = directory(current)
    if current_path:
        roots.add(repo_root(current_path))

    live = []
    pending = [0, 0]
    active_paths = children(state / 'active_pids')
    # Truncating the registry would produce incorrect counts, not just fewer rows.
    if truncated:
        raise ValueError('incomplete live-session inventory')
    for path in active_paths:
        check_time()
        owner_pid = marker_pid(path)
        if owner_pid is None:
            continue
        lease, root = leases.get(path.name, ({}, None))
        if lease.get('away_host') is not None:
            continue
        try:
            session = session_fields(state / 'sessions' / (path.name + '.json'), check_time)
        except FileNotFoundError:
            # An agent may be registering its initial snapshot. Do not invent a zero.
            if marker_pid(path) is None:
                continue
            if path.name == current_session_id and current_path:
                session = {'working_dir': str(current_path)}
            else:
                # Other fresh idle sessions have no disk metadata yet. Keep
                # their count explicitly unassigned, never guess their cwd.
                pending[0] += 1
                pending[1] += int(marker_pid(state / 'streaming_pids' / path.name) is not None)
                continue
        if session.get('is_debug') or session.get('is_canary'):
            continue
        workdir = directory(session.get('working_dir')) or root
        if workdir is None:
            raise ValueError('live session working directory unavailable')
        root = root or repo_root(workdir)
        roots.add(root)
        busy = marker_pid(state / 'streaming_pids' / path.name) is not None
        live.append((root, busy))

    # Cloud handoffs preserve absolute repository paths, rather than using one
    # fixed /workspace root. Leases locate restored roots, including idle ones.
    # Also include immediate user/project siblings so unused directories show 0.
    parents = {home}
    parents.update(home / name for name in ('workspaces', 'projects', 'repos')
                   if (home / name).is_dir())
    for root in list(roots):
        parent = root.parent
        if parent != pathlib.Path('/') and parent != home.parent:
            parents.add(parent)
    for parent in sorted(parents):
        for path in children(parent):
            if not path.name.startswith('.') and path.is_dir():
                roots.add(path.resolve())

    counts = {root: [0, 0] for root in roots}
    for root, busy in live:
        counts[root][0] += 1
        counts[root][1] += int(busy)

    def display(path):
        try:
            relative = path.relative_to(home)
            return '~' if str(relative) == '.' else '~/' + str(relative)
        except ValueError:
            return str(path)

    current_root = repo_root(current_path) if current_path else None
    ordered = sorted(counts, key=lambda root: (
        root != current_root, -counts[root][1], -counts[root][0], str(root)))
    limit = MAX_ITEMS - int(pending[0] > 0)
    if len(ordered) > limit:
        truncated = True
    entries = [dict(path=display(root), agents=counts[root][0], busy=counts[root][1])
               for root in ordered[:limit]]
    if pending[0]:
        entries.append(dict(path='(directory pending)', agents=pending[0], busy=pending[1]))
    return {'current_path': display(current_root) if current_root else None,
            'entries': entries, 'truncated': truncated}


if __name__ == '__main__':
    try:
        print(json.dumps(collect(sys.argv[1], current_session_id=sys.argv[2] if len(sys.argv) > 2 else None),
                         ensure_ascii=True, separators=(',', ':')))
    except (OSError, ValueError, TypeError, TimeoutError):
        sys.exit(1)
