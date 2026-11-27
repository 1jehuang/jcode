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

    def test_signed_early_send_and_final_callback_boundaries(self):
        early = [marker(10, 'CAPTURE', 'speech_onset'),
                 marker(40, 'DISPATCH', 'sent', extra='provisional=true final_update=false'),
                 marker(75, 'CAPTURE', 'speech_endpoint')]
        [turn] = summarize(early)
        self.assertEqual(turn['endpoint_to_first_send_ms'], -35)
        self.assertEqual(turn['onset_to_first_send_ms'], 30)
        self.assertIsNone(turn['endpoint_to_final_send_ms'])
        self.assertIsNone(turn['endpoint_to_first_speak_callback_ms'])
        [turn] = summarize(early + [marker(90, 'DISPATCH', 'interleave_sent', extra='provisional=false final_update=true'),
                                    marker(100, 'PLAYBACK', 'deferred')])
        self.assertEqual(turn['endpoint_to_first_send_ms'], -35)
        self.assertEqual(turn['endpoint_to_final_send_ms'], 15)
        self.assertEqual(turn['endpoint_to_first_speak_callback_ms'], 25)
        self.assertEqual(turn['final_send_to_first_speak_callback_ms'], 10)

    def test_synthesis_without_player_does_not_imply_playback(self):
        [turn] = summarize([marker(65, 'PLAYBACK', 'enqueue'), marker(80, 'PLAYBACK', 'synthesis_started')])
        self.assertEqual(turn['playback_boundary'], 'synthesis_started')
        self.assertIsNone(turn['synthesis_to_player_ms'])

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

    def test_capture_discard_reasons_are_allowlisted_and_final_is_separate(self):
        for raw, expected in [('queue_overflow', 'queue_overflow'), ('private-conversation', None)]:
            with self.subTest(raw=raw):
                [turn] = summarize([marker(2, 'CAPTURE', 'final_accepted'),
                                    marker(3, 'CAPTURE', 'accepted_discarded', extra=f'reason={raw}')])
                self.assertEqual((turn['delivery'], turn['reason']), ('dropped', expected))
                [turn] = summarize([marker(2, 'DISPATCH', 'sent', extra='provisional=true'),
                                    marker(3, 'CAPTURE', 'continuation_discarded', extra=f'reason={raw}')])
                self.assertEqual((turn['delivery'], turn['final_delivery'], turn['final_reason']),
                                 ('sent', 'continuation_discarded', expected))
                self.assertNotIn('private-conversation', json.dumps(turn))

    def test_capture_without_session_joins_only_unambiguous_owner(self):
        capture = marker(10, 'CAPTURE', 'final_accepted').replace('session=local-a ', '')
        a = marker(15, 'DISPATCH', 'queued', 'a')
        b = marker(20, 'DISPATCH', 'sent', 'b')
        [alone] = summarize([capture])
        self.assertIsNone(alone['session'])
        self.assertEqual(alone['delivery'], 'accepted_delivery_unavailable')
        [joined] = summarize([capture, b])
        self.assertEqual((joined['session'], joined['acceptance_to_send_ms']), ('b', 10))
        self.assertIsNone(summarize([capture, a, b], 'b')[0]['acceptance_to_send_ms'])

    def test_cli_never_echoes_content_or_embedded_markers(self):
        lines = marker(25, 'CAPTURE', 'partial_accepted', extra='transcript=secret')
        lines += '[2026-11-19 01:02:03.000] [INFO] tool input: ' + marker(26, 'DISPATCH', 'sent')
        proc = subprocess.run([sys.executable, str(Path(__file__).with_name('jarvis_latency_report.py')), '--json', '-'],
                              input=lines, text=True, capture_output=True, check=True)
        self.assertNotIn('secret', proc.stdout)
        self.assertEqual(json.loads(proc.stdout)[0]['delivery'], 'accepted_delivery_unavailable')


if __name__ == '__main__':
    unittest.main()
