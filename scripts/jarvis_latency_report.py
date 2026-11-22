#!/usr/bin/env python3
"""Read local Jcode logs and summarize Jarvis delivery without printing conversation content."""
import argparse
from collections import defaultdict
from datetime import datetime
import json
import os
from pathlib import Path
import re
import sys

# Only trust actual INFO records, not marker-looking strings embedded in other logs.
LINE = re.compile(r'^\[([\d-]+ [\d:.]+)\] \[INFO\] JARVIS_(CAPTURE|DISPATCH|PLAYBACK) (.*)$')
CLOCK_NOTE = ('Intervals use the shared process monotonic clock when both markers carry it, otherwise local log timestamps. '
              'endpoint_to_first_send_ms is signed: negative means local submission preceded the VAD endpoint. '
              'The first speak callback is the local deferred marker, not audible output.')
ACCEPT = {'partial_accepted', 'final_accepted', 'manual_accepted'}
SEND = {'sent', 'interleave_sent'}
DROP = {'queue_full', 'invalid', 'suppressed', 'hourly_limit', 'expired', 'detached',
        'capture_error', 'capture_failed', 'capture_stopped', 'paused_or_failed', 'send_failed'}


CAPTURE_DISCARD_REASONS = {'stream_aborted', 'queue_overflow', 'epoch_changed', 'stopped',
                           'capture_failed', 'paused', 'suppressed', 'permission_lost'}


def discard_reason(event):
    if event[1] == 'DISPATCH':
        return event[2]  # The caller already checked a fixed stage allowlist.
    reason = event[3].get('reason')
    return reason if reason in CAPTURE_DISCARD_REASONS else None


def records(lines):
    for line in lines:
        m = LINE.match(line.rstrip('\n'))
        if not m:
            continue
        stamp, kind, fields = m.groups()
        kv = dict(re.findall(r'(\w+)=([^\s]+)', fields))
        uid = re.fullmatch(r'(?:Some\(([0-9]+)\)|([0-9]+))', kv.get('utterance', ''))
        if not uid or not re.fullmatch(r'[a-z_]+', kv.get('stage', '')):
            continue
        session = kv.get('session')
        if (session and not re.fullmatch(r'[\w.:-]{1,160}', session)) or (uid[1] or uid[2]) == '0':
            continue
        if session == 'none':
            session = None
        try:
            at = datetime.fromisoformat(stamp)
        except ValueError:
            continue
        yield (session, uid[1] or uid[2]), (at, kind, kv['stage'], kv)


def summarize(lines, session=None):
    groups = defaultdict(list)
    for key, rec in records(lines):
        groups[key].append(rec)
    # Capture runs before the UI knows the session. Its process-randomized u64
    # is carried unchanged into dispatch. Never guess between conflicting owners.
    owners = defaultdict(set)
    for sid, uid in groups:
        if sid is not None:
            owners[uid].add(sid)
    for sid, uid in list(groups):
        if sid is None and len(owners[uid]) == 1:
            groups[next(iter(owners[uid])), uid].extend(groups.pop((None, uid)))
    result = []
    for (sid, uid), events in groups.items():
        if session is not None and sid != session:
            continue
        events.sort(key=lambda e: e[0])
        def first(stages, kind):
            return next((e for e in events if e[1] == kind and e[2] in stages), None)
        def delta(a, b, *, signed=False):
            if not a or not b:
                return None
            if all(e[3].get('monotonic_ms', '').isdigit() for e in (a, b)):
                ms = int(b[3]['monotonic_ms']) - int(a[3]['monotonic_ms'])
            else:
                ms = round((b[0] - a[0]).total_seconds() * 1000)
            return ms if signed or ms >= 0 else None
        def capture(*stages):
            return first(stages, 'CAPTURE')
        accept = first(ACCEPT, 'CAPTURE')
        queued = first({'queued'}, 'DISPATCH')
        sent = first(SEND, 'DISPATCH')
        dropped = capture('accepted_discarded') or first(DROP, 'DISPATCH')
        endpoint = capture('speech_endpoint')
        final = capture('transcription_finished')
        deferred = first({'deferred'}, 'PLAYBACK')
        playback = first({'enqueue'}, 'PLAYBACK')
        player = first({'player_started'}, 'PLAYBACK')
        synth = first({'synthesis_started'}, 'PLAYBACK')
        continuation = capture('continuation_ready')
        final_sent = next((e for e in events if e[2] in SEND and e[3].get('final_update') == 'true'), None)
        final_request = next((e for e in events if e[1] == 'DISPATCH' and e[2] in SEND
                              and (e[3].get('provisional') == 'false' or e[3].get('final_update') == 'true')), None)
        merged = first({'merged_final'}, 'DISPATCH')
        final_drop = capture('continuation_discarded') or first({'final_suppressed', 'invalid_final', 'unmatched_final'}, 'DISPATCH')
        final_delivery = ('sent' if final_sent or (merged and sent) else
                          final_drop[2] if final_drop else 'unavailable' if continuation else None)
        disposition = ('sent' if sent else 'dropped' if dropped else
                       'accepted_delivery_unavailable' if accept or queued else 'unavailable')
        result.append(dict(session=sid, utterance=uid, delivery=disposition,
                           reason=discard_reason(dropped) if dropped and not sent else None,
                           acceptance=accept[2] if accept else None,
                           final_delivery=final_delivery,
                           final_reason=discard_reason(final_drop) if final_drop and final_delivery != 'sent' else None,
                           onset_to_acceptance_ms=delta(capture('speech_onset'), accept),
                           partial_gate_to_acceptance_ms=delta(capture('partial_gate_sent'), capture('partial_accepted')),
                           endpoint_to_final_ms=delta(endpoint, final),
                           final_to_gate_ms=delta(final, capture('final_gate_decided')),
                           onset_to_first_send_ms=delta(capture('speech_onset'), sent),
                           endpoint_to_first_send_ms=delta(endpoint, sent, signed=True),
                           endpoint_to_final_send_ms=delta(endpoint, final_request),
                           endpoint_to_first_speak_callback_ms=delta(endpoint, deferred),
                           final_send_to_first_speak_callback_ms=delta(final_request, deferred),
                           acceptance_to_send_ms=delta(accept, sent),
                           queue_to_send_ms=(int(sent[3]['queue_ms']) if sent and sent[3].get('queue_ms', '').isdigit() else delta(queued, sent)),
                           endpoint_to_speech_enqueue_ms=delta(endpoint, playback),
                           send_to_deferred_ms=delta(sent, deferred),
                           playback_hold_ms=(int(playback[3]['held_ms']) if playback and playback[3].get('held_ms', '').isdigit() else delta(deferred, playback)),
                           send_to_enqueue_ms=delta(sent, playback),
                           enqueue_to_synthesis_ms=delta(playback, synth),
                           synthesis_to_player_ms=delta(synth, player),
                           endpoint_to_player_ms=delta(endpoint, player),
                           playback_boundary=('player_process_started' if player else 'synthesis_started' if synth else 'speech_enqueue' if playback else None)))
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__, epilog='A send is local submission, not proof the model received the turn. Speech enqueue and player process start do not measure audible playback. Missing markers are unavailable, not evidence of a drop. ' + CLOCK_NOTE)
    ap.add_argument('logs', nargs='*', help='Chronological daily log files, or - for stdin (default: today’s Jcode log)')
    ap.add_argument('--session', help='Only show the exact local session ID')
    ap.add_argument('--json', action='store_true', help='Emit a JSON array, using null for unavailable measurements')
    args = ap.parse_args()
    paths = args.logs or [Path(os.getenv('JCODE_HOME', Path.home() / '.jcode')) / 'logs' / f'jcode-{datetime.now():%Y-%m-%d}.log']
    def lines():
        for path in paths:
            if path == '-':
                yield from sys.stdin
            else:
                with open(path, errors='replace') as src:
                    yield from src
    try:
        rows = summarize(lines(), args.session)
    except OSError as exc:
        ap.exit(1, f'{ap.prog}: {exc}\n')
    if args.json:
        print(json.dumps(rows, indent=2))
    else:
        print('Send = local submission. Speech enqueue and player process start do not measure audible playback. Unavailable does not mean dropped.')
        print(CLOCK_NOTE)
        missing = sum(row['delivery'] == 'accepted_delivery_unavailable' for row in rows)
        dropped = sum(row['delivery'] == 'dropped' for row in rows)
        print(f'Utterances: {len(rows)}. Explicit delivery failures: {dropped}. Accepted without observed send or loss: {missing}.')
        for row in rows:
            print(' '.join(f'{k}={v if v is not None else "unavailable"}' for k, v in row.items()))
        if not rows:
            print('No matching correlated Jarvis markers.')


if __name__ == '__main__':
    main()
