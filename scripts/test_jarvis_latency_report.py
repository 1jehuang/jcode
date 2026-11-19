import json
from pathlib import Path
import subprocess
import sys
import unittest

from jarvis_latency_report import summarize


def marker(ms, kind, stage, session='local-a', uid=12, extra=''):
    return (f'[2026-11-19 01:02:03.{ms:03}] [INFO] JARVIS_{kind} session={session} '
            f'utterance={uid} stage={stage} monotonic_ms={ms} {extra}\n')


class JarvisReportTests(unittest.TestCase):
    def test_latency_stops_at_player_not_audible_audio(self):
        rows = [marker(*args) for args in [(10, 'CAPTURE', 'speech_endpoint'),
                (23, 'CAPTURE', 'transcription_finished'), (32, 'CAPTURE', 'final_gate_decided'),
                (34, 'CAPTURE', 'final_accepted'), (35, 'DISPATCH', 'queued'),
                (40, 'DISPATCH', 'interleave_sent'), (65, 'PLAYBACK', 'enqueue'),
                (80, 'PLAYBACK', 'synthesis_started'), (110, 'PLAYBACK', 'player_started')]]
        [turn] = summarize(reversed(rows))
        self.assertEqual(turn['delivery'], 'sent')
        self.assertEqual(turn['endpoint_to_final_ms'], 13)
        self.assertEqual(turn['final_to_gate_ms'], 9)
        self.assertEqual(turn['queue_to_send_ms'], 5)
        self.assertEqual(turn['endpoint_to_player_ms'], 100)
        self.assertEqual(turn['playback_boundary'], 'player_process_started')

    def test_explicit_loss_vs_missing_across_sessions(self):
        rows = [marker(20, 'CAPTURE', 'manual_accepted', sid) for sid in ['a', 'b']]
        rows.append(marker(22, 'DISPATCH', 'queue_full', 'b'))
        a, b = summarize(rows)
        self.assertEqual(a['delivery'], 'accepted_delivery_unavailable')
        self.assertIsNone(a['reason'])
        self.assertIsNone(a['endpoint_to_player_ms'])
        self.assertEqual((b['delivery'], b['reason']), ('dropped', 'queue_full'))
        self.assertEqual(summarize(rows, session='b'), [b])

    def test_lost_continuation_does_not_erase_initial_send(self):
        [row] = summarize([marker(10, 'DISPATCH', 'sent'), marker(20, 'CAPTURE', 'continuation_ready'),
                           marker(30, 'DISPATCH', 'final_suppressed')])
        self.assertEqual((row['delivery'], row['final_delivery']), ('sent', 'final_suppressed'))

    def test_cli_never_echoes_content_or_embedded_markers(self):
        lines = marker(25, 'CAPTURE', 'partial_accepted', extra='transcript=secret')
        lines += '[2026-11-19 01:02:03.000] [INFO] tool input: ' + marker(26, 'DISPATCH', 'sent')
        proc = subprocess.run([sys.executable, str(Path(__file__).with_name('jarvis_latency_report.py')), '--json', '-'],
                              input=lines, text=True, capture_output=True, check=True)
        self.assertNotIn('secret', proc.stdout)
        self.assertEqual(json.loads(proc.stdout)[0]['delivery'], 'accepted_delivery_unavailable')


if __name__ == '__main__':
    unittest.main()
