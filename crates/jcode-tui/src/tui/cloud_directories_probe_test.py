"""Focused, synthetic-only checks. Run: python3 cloud_directories_probe_test.py.

--benchmark BASELINE_PATH compares identical frozen synthetic files, never user state.
"""
import importlib.util
import json
import pathlib
import sys
import tempfile
import time
import unittest
from unittest import mock


def load(path):
    spec = importlib.util.spec_from_file_location('probe', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


probe = load(pathlib.Path(__file__).with_name('cloud_directories_probe.py'))


class ProbeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.path = self.root / 'session.json'

    def fields(self, raw):
        self.path.write_bytes(raw)
        return probe.session_fields(self.path, lambda: None)

    def test_duplicate_escaped_metadata(self):
        self.assertEqual(self.fields(br'{"working_dir":"/old","messages":[{"working_dir":"/fake"}],"working\u005fdir":"/new","is_debug":false,"is_canary":true}'),
                         dict(working_dir='/new', is_debug=False, is_canary=True))

    def test_container_duplicate_clears_scalar(self):
        result = self.fields(b'{"working_dir":"/old","working_dir":{},"is_debug":[]}')
        self.assertNotEqual(result.get('working_dir'), '/old')
        self.assertIn('is_debug', result)

    def test_invalid_snapshots(self):
        for raw in (b'', b'[]', b'{"messages":[1 2]}', b'{"messages":[1,]}',
                    b'{"a":true "b":false}', b'{"a":01}', b'{"a":"\\q"}',
                    b'{"a":1}junk', b'{"a":{"b":}}', b'{"a":true,}',
                    b'{"messages":["\xff"]}'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                self.fields(raw)

    def test_latest_complete_journal_and_partial(self):
        self.path.write_bytes(b'{"working_dir":"/snapshot"}')
        journal = self.path.with_suffix('.journal.jsonl')
        journal.write_bytes(b'{"meta":{"working_dir":"/old"}}\n'
                            b'{"messages":[1],"meta":{"working_dir":"/new","is_debug":true}}\n'
                            b'{"meta":{"working_dir":"/partial"}')
        self.assertEqual(probe.session_fields(self.path, lambda: None),
                         dict(working_dir='/new', is_debug=True, is_canary=None))
        journal.write_bytes(b'{bad}\n{"meta":null}\n')
        self.assertEqual(probe.session_fields(self.path, lambda: None), {'working_dir': '/snapshot'})

    def test_duplicate_meta_and_invalid_journal(self):
        self.path.write_bytes(b'{"working_dir":"/snapshot"}')
        self.path.with_suffix('.journal.jsonl').write_bytes(
            b'{"meta":{"working_dir":"/old"},"meta":{"working_dir":"/last"}}\n'
            b'{"meta":{"working_dir":"/bad"},"messages":[1 2]}\n')
        self.assertEqual(probe.session_fields(self.path, lambda: None)['working_dir'], '/last')

    def test_deadline(self):
        self.path.write_bytes(b'{"messages":[] }')
        def expired():
            raise TimeoutError('synthetic deadline')
        with self.assertRaises(TimeoutError):
            probe.session_fields(self.path, expired)

    def test_long_strings_unicode_nested_values_and_no_transcript_decoding(self):
        transcript = 'TRANSCRIPT_SENTINEL' + ('abc\\\"[]{}' * 20000) + '\U0001f98a\u00e9'
        raw = json.dumps({'messages': [{'content': transcript}],
                          'working_dir': '/exact/sub', 'is_debug': False},
                         ensure_ascii=False).encode()
        original = json.loads
        def scalar_only(value):
            self.assertNotIn(b'TRANSCRIPT_SENTINEL', bytes(value))
            return original(value)
        with mock.patch.object(probe.json, 'loads', side_effect=scalar_only):
            self.assertEqual(self.fields(raw), dict(working_dir='/exact/sub', is_debug=False))
            self.path.with_suffix('.journal.jsonl').write_bytes(json.dumps(
                {'messages': [{'content': transcript}], 'meta': {'working_dir': '/journal/sub'}},
                ensure_ascii=False).encode() + b'\n{"meta":')
            self.assertEqual(probe.session_fields(self.path, lambda: None),
                             dict(working_dir='/journal/sub', is_debug=None, is_canary=None))
            self.path.with_suffix('.journal.jsonl').unlink()
        for length in (probe.SCAN_WINDOW - 1, probe.SCAN_WINDOW, probe.SCAN_WINDOW + 1):
            self.assertEqual(self.fields(b'{"ignored":"' + b'a' * length + b'","working_dir":"/ok"}'),
                             {'working_dir': '/ok'})
        for offset in (-2, -1, 0, 1):
            for tail in ('\U0001f98a"\\tail', '\\"\u00e9'):
                raw = json.dumps({'ignored': 'a' * (probe.SCAN_WINDOW + offset) + tail,
                                  'working_dir': '/boundary'}, ensure_ascii=False).encode()
                self.assertEqual(self.fields(raw), {'working_dir': '/boundary'})
        nested = 20 * b'[' + b'"ok"' + 20 * b']'
        self.assertEqual(self.fields(b'{"ignored":' + nested + b',"working_dir":"/ok"}'), {'working_dir': '/ok'})
        with self.assertRaises(ValueError):
            self.fields(b'{"ignored":' + 140 * b'[' + b'0' + 140 * b']' + b'}')

    def test_checkpoint_inside_large_string_and_whitespace(self):
        for raw in (b'{"messages":["' + b'x' * (probe.SCAN_WINDOW * 8) + b'"]}',
                    b' ' * (probe.SCAN_WINDOW * 8) + b'{}'):
            self.path.write_bytes(raw)
            calls = 0
            def expire_during_scan():
                nonlocal calls
                calls += 1
                if calls == 6:
                    raise TimeoutError('bounded checkpoint')
            with self.assertRaises(TimeoutError):
                probe.session_fields(self.path, expire_during_scan)

    def test_journal_bounds_include_older_records_and_newlines(self):
        self.path.write_bytes(b'{"working_dir":"/snapshot"}')
        journal = self.path.with_suffix('.journal.jsonl')
        with mock.patch.object(probe, 'MAX_JSON_BYTES', 40):
            for raw in (b' ' * 41 + b'\n{"meta":{}}\n', b' ' * 40 + b'\n', b'x' * 41):
                journal.write_bytes(raw)
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    probe.session_fields(self.path, lambda: None)
        journal.write_bytes(b'{"meta":{"working_dir":"/first"}}\n{"meta":null}\n\n')
        self.assertEqual(probe.session_fields(self.path, lambda: None)['working_dir'], '/first')

    def test_pending_session_and_current_metadata_fallback(self):
        home = self.root / 'home'
        home.mkdir()
        state = self.root / 'state'
        (state / 'active_pids').mkdir(parents=True)
        (state / 'streaming_pids').mkdir()
        (state / 'active_pids' / 'new').write_text('1')
        (state / 'streaming_pids' / 'new').write_text('1')
        result = probe.collect(str(home), home, state, alive=lambda _: True)
        self.assertIn(dict(path='(directory pending)', agents=1, busy=1), result['entries'])
        result = probe.collect(str(home), home, state, alive=lambda _: True, current_session_id='new')
        self.assertEqual(result['entries'], [dict(path='~', agents=1, busy=1)])

    def test_collect_retains_four_second_deadline(self):
        with mock.patch.object(probe.time, 'monotonic', side_effect=[0, 4.000001]):
            with self.assertRaises(TimeoutError):
                probe.collect(str(self.root), self.root, self.root / 'synthetic-state')

    def test_collect_exact_counts_flags_and_alive(self):
        home = self.root / 'home'
        repo = home / 'repo'
        sub = repo / 'sub'
        sub.mkdir(parents=True)
        (repo / '.git').mkdir()
        state = self.root / 'state'
        for name in ('sessions', 'active_pids', 'streaming_pids'):
            (state / name).mkdir(parents=True)
        for sid, pid, flags in [('idle', 1, {}), ('busy', 2, {}), ('debug', 3, {'is_debug': True}),
                                ('canary', 4, {'is_canary': True}), ('dead', 9, {})]:
            (state / 'active_pids' / sid).write_text(str(pid))
            (state / 'sessions' / (sid + '.json')).write_text(json.dumps(dict(working_dir=str(sub), **flags)))
        (state / 'streaming_pids' / 'busy').write_text('2')
        result = probe.collect(str(sub), home, state, alive=lambda pid: pid != 9)
        self.assertEqual(result, dict(current_path='~/repo', entries=[dict(path='~/repo', agents=2, busy=1)], truncated=False))


def benchmark(baseline):
    old = load(baseline)
    with tempfile.TemporaryDirectory(prefix='jcode-probe-synthetic-') as temp:
        path = pathlib.Path(temp) / 'session.json'
        # Fixed transcript shape with nested tool payloads and escaped punctuation.
        message = json.dumps({'role': 'assistant', 'content': [{'type': 'tool', 'text': 'hello \\\"[]{} metadata ' * 12}], 'usage': {'input': 42, 'output': 17}}, separators=(',', ':')).encode()
        payload = b'{"working_dir":"/synthetic/exact/sub","is_debug":false,"is_canary":false,"messages":[' + b','.join([message] * 256000) + b']}'
        path.write_bytes(payload)
        journal = path.with_suffix('.journal.jsonl')
        for kind in ('snapshot', 'journal'):
            if kind == 'journal':
                entry = b'{"meta":{"working_dir":"/synthetic/exact/sub","is_debug":false,"is_canary":false},"messages":[' + b','.join([message] * 16000) + b']}\n'
                journal.write_bytes(entry * 16 + b'{"meta":')
            results = []
            for label, module in [('before', old), ('after', probe)]:
                started = time.perf_counter()
                result = module.session_fields(path, lambda: None)
                results.append(result)
                print(json.dumps(dict(kind=kind, implementation=label, bytes=(path if kind == 'snapshot' else journal).stat().st_size, seconds=round(time.perf_counter()-started, 6), output=result), sort_keys=True))
            assert results[0] == results[1], 'before/after metadata mismatch'


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--benchmark':
        benchmark(sys.argv[2])
    else:
        unittest.main()
