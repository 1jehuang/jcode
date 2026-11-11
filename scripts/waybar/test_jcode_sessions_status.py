import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import types
import unittest
from unittest.mock import patch


class CloudCountsTests(unittest.TestCase):
    def setUp(self):
        self.module = runpy.run_path(str(Path(__file__).with_name('jcode-sessions-status')))
        self.scope = self.module['_cloud_probe'].__globals__
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def marker(self, directory, session, pid=None):
        path = self.root / directory / session
        path.parent.mkdir(exist_ok=True)
        path.write_text(str(os.getpid() if pid is None else pid))

    def probe(self):
        return self.module['_cloud_probe'](self.root)

    def test_counts_sessions_not_shared_daemon_pids(self):
        self.marker('active_pids', 'root')
        self.marker('active_pids', 'worker')
        self.marker('streaming_pids', 'worker')
        self.marker('internal_pids', 'worker')
        self.assertEqual(self.probe(), {'streaming': 1, 'alive': 2})

    def test_stale_and_invalid_markers_are_ignored(self):
        for pid in (0, -1, 2147483648, 'invalid'):
            self.marker('active_pids', str(pid), pid)
        self.marker('active_pids', 'live')
        self.marker('streaming_pids', 'live', 0)
        self.marker('streaming_pids', 'orphan')
        self.assertEqual(self.probe(), {'streaming': 0, 'alive': 1})

    def test_dead_owner_is_not_alive(self):
        self.marker('active_pids', 'dead')
        self.marker('streaming_pids', 'dead')
        with patch.dict(self.scope, {'_pid_is_live': lambda path: False}):
            self.assertEqual(self.probe(), {'streaming': 0, 'alive': 0})

    def test_no_markers_means_zero(self):
        self.assertEqual(self.probe(), {'streaming': 0, 'alive': 0})

    def test_cloud_streaming_controls_color_without_local_sessions(self):
        output = self.module['_build_output'](0, 0, 0, {
            'streaming': 2, 'alive': 5, 'detail': 'cloud'})
        self.assertEqual(output['class'], 'streaming')
        self.assertIn('2/5', output['text'])
        self.assertIn('Cloud: 2 streaming / 5 alive', output['tooltip'])

    def test_unknown_does_not_display_zero(self):
        output = self.module['_build_output'](0, 0, 0)
        self.assertIn('?/?', output['text'])
        self.assertNotEqual(output['class'], 'hidden')

    def test_sleeping_host_is_not_connected_or_woken(self):
        calls = []
        def run(args):
            calls.append(args)
            if 'ephemeral' in args:
                return types.SimpleNamespace(stdout='[]')
            return types.SimpleNamespace(stdout='Jcode Cloud machine `jcode-cloud`: stopped')
        with patch.dict(self.scope, {'_run': run}):
            self.assertEqual(self.module['_query_cloud_counts']()['alive'], 0)
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(call[0] != 'ssh' for call in calls))

    def test_managed_and_ephemeral_counts_are_summed(self):
        calls = []
        def run(args):
            calls.append(args)
            if args[0] == 'ssh':
                return types.SimpleNamespace(stdout='{"streaming": 2, "alive": 5}')
            if 'ephemeral' in args:
                return types.SimpleNamespace(stdout=json.dumps([
                    {'id': 'cm_abc123', 'state': 'running'},
                    {'id': 'cm_dead', 'state': 'terminated'},
                ]))
            return types.SimpleNamespace(stdout='Jcode Cloud machine `jcode-cloud`: running')
        with patch.dict(self.scope, {'_run': run}):
            result = self.module['_query_cloud_counts']()
        self.assertEqual((result['streaming'], result['alive']), (4, 10))
        self.assertEqual(sum(call[0] == 'ssh' for call in calls), 2)

    def test_connection_failure_is_unknown(self):
        def run(args):
            raise subprocess.TimeoutExpired(args, 20)
        with patch.dict(self.scope, {'_run': run}):
            self.assertIsNone(self.module['_query_cloud_counts']()['alive'])


if __name__ == '__main__':
    unittest.main()
