"""Tests for the exact read-only collector embedded in the cloud SSH probe."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

PROBE = Path(__file__).resolve().parents[1] / 'crates/jcode-tui/src/tui/cloud_directories_probe.py'
spec = importlib.util.spec_from_file_location('cloud_directories_probe', PROBE)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class CloudDirectoriesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name) / 'home'
        self.home.mkdir()
        self.state = self.home / '.jcode'
        for name in ['active_pids', 'streaming_pids', 'internal_pids', 'sessions', 'session_leases']:
            (self.state / name).mkdir(parents=True)
        self.project = self.home / 'project'
        (self.project / '.git').mkdir(parents=True)
        self.other = self.home / 'other'
        self.other.mkdir()

    def session(self, name, cwd=None, pid=123, busy=False, **metadata):
        (self.state / 'active_pids' / name).write_text(str(pid))
        if busy:
            (self.state / 'streaming_pids' / name).write_text(str(pid))
        (self.state / 'sessions' / (name + '.json')).write_text(json.dumps({
            'working_dir': str(cwd or self.project), **metadata}))

    def collect(self, current=None):
        return probe.collect(str(current or self.project), self.home, self.state,
                             alive=lambda pid: pid == 123)

    def rows(self, snapshot):
        return {row['path']: (row['agents'], row['busy']) for row in snapshot['entries']}

    def test_shared_pid_counts_sessions_and_idle_directories(self):
        self.session('root', busy=True)
        nested = self.project / 'src'
        nested.mkdir()
        self.session('worker', nested, busy=True, parent_id='root')
        (self.state / 'internal_pids' / 'worker').touch()
        self.session('idle', self.other)
        (self.home / 'unused').mkdir()
        snapshot = self.collect(nested)
        self.assertEqual(snapshot['current_path'], '~/project')
        self.assertEqual(snapshot['entries'][0]['path'], '~/project')
        self.assertEqual(self.rows(snapshot)['~/project'], (2, 2))
        self.assertEqual(self.rows(snapshot)['~/other'], (1, 0))
        self.assertEqual(self.rows(snapshot)['~/unused'], (0, 0))
        self.assertNotIn('~/project/src', self.rows(snapshot))

    def test_zero_agent_projects_in_workspace_containers(self):
        for container in ('workspaces', 'projects', 'repos'):
            project = self.home / container / 'unused'
            project.mkdir(parents=True)
        rows = self.rows(self.collect())
        for container in ('workspaces', 'projects', 'repos'):
            self.assertEqual(rows['~/' + container + '/unused'], (0, 0))

    def test_dead_debug_and_moved_sessions_are_not_agents(self):
        self.session('dead', pid=999, busy=True)
        self.session('debug', is_debug=True)
        self.session('test', is_canary=True)
        self.session('away', busy=True)
        (self.state / 'session_leases' / 'away.json').write_text(json.dumps({
            'away_host': 'local', 'repo_root': str(self.project)}))
        self.assertEqual(self.rows(self.collect())['~/project'], (0, 0))

    def test_stale_streaming_marker_is_idle(self):
        self.session('idle')
        (self.state / 'streaming_pids' / 'idle').write_text('999')
        self.assertEqual(self.rows(self.collect())['~/project'], (1, 0))

    def test_restored_absolute_roots_and_zero_agent_siblings(self):
        restored = Path(self.temp.name) / 'restored' / 'repo'
        restored.mkdir(parents=True)
        sibling = restored.parent / 'empty'
        sibling.mkdir()
        (self.state / 'session_leases' / 'old.json').write_text(json.dumps({
            'repo_root': str(restored), 'away_host': 'local'}))
        rows = self.rows(self.collect())
        self.assertEqual(rows[str(restored)], (0, 0))
        self.assertEqual(rows[str(sibling)], (0, 0))

    def test_missing_metadata_is_pending_but_corruption_is_failure(self):
        (self.state / 'active_pids' / 'starting').write_text('123')
        self.assertEqual(self.rows(self.collect())['(directory pending)'], (1, 0))
        snapshot = probe.collect(str(self.project), self.home, self.state,
                                 alive=lambda pid: pid == 123, current_session_id='starting')
        self.assertEqual(self.rows(snapshot)['~/project'], (1, 0))
        self.assertNotIn('(directory pending)', self.rows(snapshot))
        (self.state / 'sessions' / 'starting.json').write_text('{')
        with self.assertRaises(ValueError):
            self.collect()

    def test_filenames_and_symlink_aliases_are_data_not_commands(self):
        odd = self.home / "雲 ' $(not-a-command)\nproject"
        odd.mkdir()
        (self.home / 'alias').symlink_to(odd, target_is_directory=True)
        self.session('odd', odd)
        rows = self.rows(self.collect())
        self.assertEqual(rows['~/' + odd.name], (1, 0))
        self.assertNotIn('~/alias', rows)

    def test_real_script_transport_uses_environment_home_and_live_pid(self):
        self.session('real', pid=os.getpid(), busy=True)
        env = dict(os.environ, HOME=str(self.home), JCODE_HOME=str(self.state),
                   PYTHONDONTWRITEBYTECODE='1')
        result = subprocess.run(['python3', str(PROBE), str(self.project)],
                                env=env, capture_output=True, check=True, timeout=8)
        snapshot = json.loads(result.stdout)
        self.assertEqual(self.rows(snapshot)['~/project'], (1, 1))

    def test_large_snapshot_skips_transcript_and_nested_metadata(self):
        self.session('large')
        path = self.state / 'sessions' / 'large.json'
        # Exercise the real >32MiB failure found on the persistent cloud VM.
        with path.open('w') as stream:
            stream.write('{"messages":[{"working_dir":"/wrong","is_debug":true,"content":"')
            stream.write('x' * (33 * 1024 * 1024))
            stream.write('"}],"working_dir":')
            json.dump(str(self.project), stream)
            stream.write(',"is_debug":false,"is_canary":false}')
        self.assertEqual(self.rows(self.collect())['~/project'], (1, 0))

    def test_journal_metadata_is_newer_than_snapshot(self):
        self.session('journal')
        journal = self.state / 'sessions' / 'journal.journal.jsonl'
        journal.write_text(json.dumps({'meta': {'working_dir': str(self.other),
                                               'is_debug': False, 'is_canary': False}})
                           + '\n' + '{"meta":')
        rows = self.rows(self.collect())
        self.assertEqual(rows['~/other'], (1, 0))
        self.assertEqual(rows['~/project'], (0, 0))

    def test_nested_fields_and_escaped_transcript_cannot_spoof_metadata(self):
        self.session('nested', messages=[{'working_dir': '/wrong', 'is_debug': True,
                                         'content': '{"working_dir":"/wrong"} \\"'}])
        self.assertEqual(self.rows(self.collect())['~/project'], (1, 0))

    def test_live_registry_limit_is_failure_not_partial_counts(self):
        old_limit = probe.MAX_ITEMS
        probe.MAX_ITEMS = 1
        self.addCleanup(setattr, probe, 'MAX_ITEMS', old_limit)
        self.session('one')
        self.session('two')
        with self.assertRaises(ValueError):
            self.collect()


if __name__ == '__main__':
    unittest.main()
