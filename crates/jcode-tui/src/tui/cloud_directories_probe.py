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
# JSON strings are skipped in C, including escaped quotes and embedded tool JSON.
# The parser only decodes top-level scalar metadata, never transcript contents.
TOKENS = re.compile(rb'"[^"\\]*(?:\\.[^"\\]*)*"|[{}\[\],:]|true|false|null|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?')
FIELDS = {'working_dir', 'is_debug', 'is_canary'}


def session_fields(path, check_time):
    # Large sessions journal metadata without rewriting the snapshot. The newest
    # complete record is authoritative, exactly as load_startup_stub handles it.
    journal = path.with_suffix('.journal.jsonl')
    try:
        latest = None
        with open(journal, 'rb') as stream:
            while True:
                check_time()
                line = stream.readline(MAX_JSON_BYTES + 1)
                if not line:
                    break
                if len(line) > MAX_JSON_BYTES:
                    raise ValueError('oversized journal record')
                try:
                    entry = json.loads(line)
                    meta = entry.get('meta')
                    if isinstance(meta, dict):
                        latest = {key: meta.get(key) for key in FIELDS}
                except (ValueError, AttributeError):
                    continue  # tolerate a partial final append, like journal replay
        if latest is not None:
            return latest
    except FileNotFoundError:
        pass

    with open(path, 'rb') as stream:
        if os.fstat(stream.fileno()).st_size == 0:
            raise ValueError('empty session snapshot')
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            stack = []
            result = {}
            key = None
            expect_key = False
            expect_value = False
            end = 0
            for index, token in enumerate(TOKENS.finditer(data)):
                if index % 256 == 0:
                    check_time()
                if data[end:token.start()].strip():
                    raise ValueError('invalid session snapshot')
                end = token.end()
                first = data[token.start():token.start() + 1]
                if first in (b'{', b'['):
                    stack.append(first)
                    if len(stack) == 1:
                        if first != b'{':
                            raise ValueError('session must be an object')
                        expect_key = True
                    expect_value = False
                elif first in (b'}', b']'):
                    if not stack or stack.pop() != (b'{' if first == b'}' else b'['):
                        raise ValueError('unbalanced session snapshot')
                    if not stack:
                        break
                elif len(stack) == 1:
                    if first == b',':
                        expect_key = True
                    elif first == b':':
                        expect_value = True
                    elif expect_key:
                        if first != b'"' or token.end() - token.start() > 1024:
                            raise ValueError('invalid session field')
                        key = json.loads(token.group())
                        expect_key = False
                    elif expect_value:
                        if key in FIELDS:
                            if token.end() - token.start() > 16384:
                                raise ValueError('oversized session field')
                            result[key] = json.loads(token.group())
                        expect_value = False
            check_time()
            if stack or data[end:].strip():
                raise ValueError('incomplete session snapshot')
            return result


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
