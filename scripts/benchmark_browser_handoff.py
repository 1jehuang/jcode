#!/usr/bin/env python3
"""Paired browser benchmark. Stdlib only. See benchmark_browser_handoff.md.

--self-test never launches Jcode or a browser. Normal runs require a coordinator-
owned disposable browser tab and an explicitly selected built binary/model.
"""
from __future__ import annotations

import argparse
import hashlib
import fcntl
import json
import os
from pathlib import Path
import secrets
import signal
import statistics
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit


TASKS = {
    'navigation': {'discovery': ('Browser guide', 'Navigation examples'),
                   'heldout': ('Storage guide', 'Backup examples')},
    'search': {'discovery': ('cedar', 'tools'), 'heldout': ('maple', 'supplies')},
    'form': {'discovery': ('Alex Example', 'alex@example.invalid', 'Please reserve the blue sample.'),
             'heldout': ('Robin Sample', 'robin@example.invalid', 'Please reserve the green sample.')},
}


def task_prompt(task, phase):
    values = TASKS[task][phase]
    if task == 'navigation':
        return f'Navigate through Browse guides to {values[0]}, then {values[1]}, then View navigation receipt.'
    if task == 'search':
        return (f'Search the catalog for {values[0]}, filter Category to {values[1]}, submit Search, '
                'open the matching item, then Specifications, then View navigation receipt.')
    return (f'Open Sample request. Fill Name with {values[0]!r}, Email with {values[1]!r}, '
            f'and Notes with {values[2]!r}. Select Delivery as pickup, check I confirm, '
            'then submit Request sample and open View navigation receipt. This is a synthetic localhost form with no external writes.')


class Fixture:
    def __init__(self):
        self.trials = {}
        self.lock = threading.Lock()
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                self.handle_request(False)

            def do_POST(self):
                self.handle_request(True)

            def handle_request(self, post):
                parts = urlsplit(self.path).path.strip('/').split('/')
                if len(parts) != 2:
                    self.send_error(404)
                    return
                token, step = parts
                with fixture.lock:
                    state = fixture.trials.get(token)
                    if state is None:
                        self.send_error(404)
                        return
                    query = parse_qs(urlsplit(self.path).query)
                    if post:
                        query = parse_qs(self.rfile.read(int(self.headers.get('Content-Length', '0'))).decode())
                    state['requests'].append({'step': step, 'method': self.command, 'fields': query})
                    root = '/' + token
                    body = None
                    pages = {
                        'start': ('Jcode documentation', 'guides', 'Browse guides'),
                        'guides': ('Guides', 'browser', state['values'][0]),
                        'browser': (state['values'][0], 'navigation', state['values'][1]),
                        'navigation': ('Navigation examples', 'receipt', 'View navigation receipt'),
                    }
                    if not post and state['task'] == 'search' and step == 'start':
                        body = f'''<h1>Catalog</h1><form action="{root}/results">
<label>Search catalog <input name="q"></label><label>Category <select name="category">
<option>all</option><option>tools</option><option>supplies</option></select></label>
<button>Search</button></form>'''
                    elif not post and step == 'results' and state['task'] == 'search':
                        state['search_valid'] = query == {'q': [state['values'][0]], 'category': [state['values'][1]]}
                        body = (f'<h1>Results</h1><a href="{root}/item">{state["values"][0]} {state["values"][1]}</a>'
                                if state['search_valid'] else '<h1>No matching results</h1>')
                    elif not post and step == 'item' and state['task'] == 'search' and state['search_valid']:
                        body = f'<h1>Item details</h1><a href="{root}/navigation">Specifications</a>'
                    elif not post and state['task'] == 'form' and step == 'start':
                        body = f'<h1>Samples</h1><a href="{root}/request">Sample request</a>'
                    elif not post and state['task'] == 'form' and step == 'request':
                        body = f'''<h1>Sample request</h1><form method="post" action="{root}/submit">
<label>Name <input name="name" required></label><label>Email <input name="email" type="email" required></label>
<label>Notes <textarea name="notes"></textarea></label><label>Delivery <select name="delivery">
<option>mail</option><option>pickup</option></select></label>
<label><input type="checkbox" name="confirm" value="yes">I confirm</label><button>Request sample</button></form>'''
                    elif post and step == 'submit' and state['task'] == 'form':
                        expected = dict(zip(('name', 'email', 'notes'), ([v] for v in state['values'])))
                        expected.update(delivery=['pickup'], confirm=['yes'])
                        state['form_valid'] = query == expected
                        body = (f'<a href="{root}/receipt">View navigation receipt</a>' if state['form_valid']
                                else '<h1>Invalid sample request</h1>')
                    elif not post and step in pages:
                        title, target, label = pages[step]
                        body = f'<h1>{title}</h1><a href="{root}/{target}">{label}</a>'
                    elif not post and step == 'receipt' and (
                            state['task'] == 'navigation' or state.get('search_valid') or state.get('form_valid')):
                        state['receipt_page_served'] = True
                        receipt = state['receipt']
                        body = f'''<h1 id="result">Navigation complete</h1>
<p>Receipt: {receipt}</p>
<script>requestAnimationFrame(() => {{
if (document.querySelector('#result').textContent === 'Navigation complete')
fetch('{root}/visible', {{method:'POST'}});
}});
addEventListener('pagehide', () => navigator.sendBeacon('{root}/left', ''));
</script>'''
                    elif post and step == 'left':
                        state['confirmation_dom_observed'] = False
                        body = 'ok'
                    elif post and step == 'visible':
                        state['confirmation_dom_observed'] = True
                        state['visible_at'] = time.monotonic()
                        body = 'ok'
                    if body is None:
                        self.send_error(404)
                        return
                payload = ('<!doctype html><meta charset="utf-8"><title>Jcode isolated browser fixture</title>' + body).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def add(self, task="navigation", phase="discovery"):
        token = secrets.token_hex(12)
        with self.lock:
            self.trials[token] = {'task': task, 'phase': phase, 'values': TASKS[task][phase],
                                  'search_valid': False, 'form_valid': False, 'receipt': secrets.token_hex(5), 'requests': [],
                                  'receipt_page_served': False, 'confirmation_dom_observed': False}
        return token, f'http://127.0.0.1:{self.server.server_port}/{token}/start'

    def state(self, token):
        with self.lock:
            return json.loads(json.dumps(self.trials[token]))

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def server_outcome(state):
    expected = {'navigation': ['start', 'guides', 'browser', 'navigation', 'receipt'],
                'search': ['start', 'results', 'item', 'navigation', 'receipt'],
                'form': ['start', 'request', 'submit', 'receipt']}[state['task']]
    cursor = 0
    for request in state['requests']:
        if cursor < len(expected) and request['step'] == expected[cursor]:
            cursor += 1
    valid_fields = (state['task'] == 'navigation' or
                    state['task'] == 'search' and state['search_valid'] or
                    state['task'] == 'form' and state['form_valid'])
    return bool(cursor == len(expected) and state['receipt_page_served'] and valid_fields)


def extract_trace(text):
    """Use actual tool input/exec/done events, never assistant prose mentions."""
    calls, current, final, errors = {}, None, '', []
    telemetry = {'model_call_count': None, 'usage_tokens': None, 'usage_tokens_scope': 'last reported snapshot, not task total', 'actual_cost_usd': None, 'token_events': []}
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if not isinstance(event, dict):
            continue
        kind = event.get('type')
        if kind == 'tokens':
            telemetry['token_events'].append(event)
        elif kind == 'done':
            final = event.get('text', final)
            telemetry['usage_tokens'] = event.get('usage')
            telemetry['model_call_count'] = event.get('model_call_count')
            telemetry['actual_cost_usd'] = event.get('actual_cost_usd')
        elif kind == 'tool_start':
            current = event['id']
            calls[current] = {'id': current, 'name': event['name'], 'input_text': '', 'executed': False}
        elif kind == 'tool_input' and current in calls:
            calls[current]['input_text'] += event.get('delta', '')
        elif kind in ('tool_exec', 'tool_done'):
            call = calls.setdefault(event['id'], {'id': event['id'], 'name': event['name'], 'input_text': ''})
            call['executed'] = True
            if kind == 'tool_done':
                call['error'] = event.get('error')
                try:
                    output = json.loads(event.get('output', '{}'))
                    call['decision_provider'] = output.get('decision_provider') if isinstance(output, dict) else None
                    if isinstance(output, dict):
                        call['handoff_status'] = output.get('status')
                        call['timing_instrumentation'] = timing_fields(output)
                        steps = output.get('action_trace', [])
                        call['handoff_executed_steps'] = sum(
                            isinstance(step, dict) and step.get('status') == 'executed'
                            for step in steps) if isinstance(steps, list) else 0
                except (ValueError, TypeError):
                    call['decision_provider'] = None
        elif kind == 'text_delta':
            final += event.get('text', '')
        elif kind == 'text_replace':
            final = event.get('text', '')
        elif kind == 'error':
            errors.append(event)
    browser = []
    other = []
    for call in calls.values():
        try:
            value = json.loads(call.pop('input_text'))
        except (ValueError, TypeError):
            value = {}
        if call['name'].split('.')[-1] == 'browser':
            call['action'] = value.get('action') if isinstance(value, dict) else None
            browser.append(call)
        elif call.get('executed'):
            other.append(call['name'])
    return {'browser_calls': browser, 'other_tools': other, 'final_text': final, 'errors': errors,
            'parent_tool_count': sum(bool(c.get('executed')) for c in calls.values()), **telemetry}


def timing_fields(value, path=''):
    """Keep instrumented timings with paths/units intact, never infer missing spans."""
    found = {}
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = f'{path}.{key}' if path else key
            if any(word in key.lower() for word in ('timing', 'elapsed', 'duration', 'latency')):
                found[child_path] = child
            else:
                found.update(timing_fields(child, child_path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            found.update(timing_fields(child, f'{path}[{index}]'))
    return found


def handoff_metrics(trace):
    calls = [call for call in trace['browser_calls']
             if call.get('executed') and call.get('action') == 'handoff']
    effective = [call for call in calls if not call.get('error') and
                 (call.get('handoff_status') == 'done' or call.get('handoff_executed_steps', 0) > 0)]
    return {'handoff_effective': bool(effective),
            'handoff_done_calls': sum(call.get('handoff_status') == 'done' and not call.get('error')
                                      for call in calls),
            'handoff_executed_steps': sum(call.get('handoff_executed_steps', 0) for call in calls),
            'handoff_statuses': [call.get('handoff_status') for call in calls]}


def terminate(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def environment(runtime, mode="normal"):
    env = dict(os.environ)
    # Retain normal credentials/config, but do not inherit another agent's routing.
    for key in ('JCODE_SOCKET', 'JCODE_SESSION_ID', 'JCODE_PARENT_SESSION_ID'):
        env.pop(key, None)
    env['JCODE_RUNTIME_DIR'] = str(runtime)
    env['JCODE_BROWSER_HANDOFF_DISABLED'] = '1' if mode == 'direct' else '0'
    # Readiness uses server:info, which is gated even in an isolated home.
    env['JCODE_DEBUG_CONTROL'] = '1'
    return env


def start_server(binary, socket, env, log):
    process = subprocess.Popen([binary, '--no-update', '--no-selfdev', '--socket', str(socket),
                                'serve', '--server-name', 'browser-handoff-benchmark'],
                               stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
    try:
        deadline = time.monotonic() + 30
        last_probe = 'No readiness probe completed'
        while time.monotonic() < deadline and process.poll() is None:
            try:
                probe = subprocess.run([binary, '--no-update', '--no-selfdev',
                                        'debug', '--socket', str(socket), 'server:info'], env=env, capture_output=True, timeout=2)
                last_probe = (probe.stderr or probe.stdout).decode(errors='replace')[-2000:]
                if probe.returncode == 0:
                    return process
            except subprocess.TimeoutExpired:
                last_probe = 'Readiness probe timed out'
            time.sleep(.2)
        raise RuntimeError(f'Isolated daemon did not become ready. Inspect {log.name}. '
                           f'Last readiness response: {last_probe}')
    except BaseException:
        terminate(process)
        raise


def run_trial(args, fixture, runtime, output, index, mode, task="navigation", phase="discovery"):
    token, url = fixture.add(task, phase)
    trial_dir = output / f'{index:02d}-{mode}'
    trial_dir.mkdir()
    workspace = trial_dir / 'workspace'
    workspace.mkdir()
    prompt = (f'Use the browser in tab {args.tab_id} to visit {url}. ' + task_prompt(task, phase) +
              ' Tell me the receipt displayed on the final page and leave that page open. '
              'Use only the browser tool for this task. Stay in this tab.')
    if mode == 'direct':
        prompt += ' Do not use browser action="handoff". Complete the task using direct browser actions only.'
    elif mode == 'jev':
        prompt += ' Delegate the whole browser task to Jev using browser action="handoff", with this tab and all supplied task text.'
    (trial_dir / 'prompt.txt').write_text(prompt)
    socket = runtime / 'server.sock'
    env = environment(runtime, mode)
    command = [args.binary, '--no-update', '--no-selfdev', '--socket', str(socket),
               '--model', args.model, '-C', str(workspace)]
    if args.provider:
        command += ['--provider', args.provider]
    command += ['run', '--ndjson', prompt]
    started = time.monotonic()
    timed_out = False
    with (trial_dir / 'transcript.ndjson').open('w') as stdout, (trial_dir / 'stderr.log').open('w') as stderr:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=env, start_new_session=True)
        try:
            process.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
        finally:
            terminate(process)
    elapsed = time.monotonic() - started
    trace = extract_trace((trial_dir / 'transcript.ndjson').read_text())
    state = fixture.state(token)
    actions = [c['action'] for c in trace['browser_calls'] if c.get('executed')]
    unknown = any(a is None for a in actions)
    handoff = 'handoff' in actions
    expected_url = url.rsplit('/', 1)[0] + '/receipt'
    validation_error = None
    try:
        checked = bridge_call('evaluate', args.tab_id, frameId=0,
                              script='return {correct: location.href === ' + json.dumps(expected_url) +
                              ' && document.body.innerText.includes(' + json.dumps(state['receipt']) +
                              ') && document.querySelector("#result")?.textContent === "Navigation complete"};')
        final_dom_correct = checked.get('result', {}).get('correct') is True
    except (OSError, ValueError, subprocess.SubprocessError, AttributeError):
        final_dom_correct = False
        validation_error = 'Independent scoped DOM probe failed'
    server_correct = server_outcome(state)
    correct = server_correct and state['confirmation_dom_observed'] and final_dom_correct
    receipt_correct = state['receipt'] in trace.pop('final_text')
    providers = [c.get('decision_provider') for c in trace['browser_calls']
                 if c.get('executed') and c['action'] == 'handoff']
    provider_valid = all(p == args.expected_handoff_provider for p in providers)
    compliant = (mode != 'jev' or handoff_metrics(trace)['handoff_effective']) and provider_valid and not trace['other_tools'] and not unknown and (mode != 'direct' or not handoff)
    result = {'type': 'trial', 'pair': index, 'mode': mode, 'task': task, 'phase': phase, 'model': args.model,
              'direct_guard_requested': mode == 'direct',
              'parent_tool_count': trace['parent_tool_count'], 'model_call_count': trace['model_call_count'],
              'usage_tokens': trace['usage_tokens'], 'usage_tokens_scope': trace['usage_tokens_scope'], 'actual_cost_usd': trace['actual_cost_usd'],
              'elapsed_seconds': round(elapsed, 3), 'exit_code': process.returncode,
              'timed_out': timed_out, 'handoff_used': handoff, **handoff_metrics(trace),
              'decision_providers': providers, 'expected_handoff_provider': args.expected_handoff_provider, 'actions': actions,
              'trace_complete': not unknown and bool(actions), 'correct_page_state': correct,
              'server_outcome_correct': server_correct, 'final_dom_correct': final_dom_correct, 'validation_error': validation_error,
              'receipt_correct': receipt_correct, 'protocol_compliant': compliant,
              'valid_success': bool(correct and receipt_correct and compliant and actions
                                    and not timed_out and process.returncode == 0 and not trace['errors']),
              'fixture': state, 'trace': trace,
              'seconds_to_confirmation': round(state['visible_at'] - started, 3) if 'visible_at' in state else None}
    (trial_dir / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def summarize(results):
    pairs = {}
    for r in results:
        pairs.setdefault((r.get('phase', 'discovery'), r.get('task', 'navigation'), r['pair']), {})[r['mode']] = r
    comparison = 'jev' if any(r['mode'] == 'jev' for r in results) else 'normal'
    complete = [p for p in pairs.values() if comparison in p and 'direct' in p]
    eligible = [p for p in complete if all(p[m]['valid_success'] for m in (comparison, 'direct'))
                and p[comparison]['handoff_used'] and p[comparison].get('handoff_effective', False)
                and not p['direct']['handoff_used']]
    ratios = [p['direct']['elapsed_seconds'] / p[comparison]['elapsed_seconds'] for p in eligible]
    all_ratios = [p['direct']['elapsed_seconds'] / p[comparison]['elapsed_seconds'] for p in complete
                  if p[comparison]['elapsed_seconds'] > 0]
    arms = {}
    for mode in (comparison, 'direct'):
        rows = [r for r in results if r['mode'] == mode]
        times = [r['elapsed_seconds'] for r in rows]
        arms[mode] = {'attempts': len(rows), 'successes': sum(r['valid_success'] for r in rows),
                      'success_rate': sum(r['valid_success'] for r in rows) / len(rows) if rows else None,
                      'failures': sum(not r['valid_success'] for r in rows),
                      'timeouts': sum(r.get('timed_out', False) for r in rows),
                      'protocol_violations': sum(not r.get('protocol_compliant', True) for r in rows),
                      'handoff_violations': sum(r['handoff_used'] for r in rows) if mode == 'direct' else 0,
                      'all_attempt_seconds': times, 'median_all_attempt_seconds': statistics.median(times) if times else None}
    ratio = statistics.median(ratios) if ratios else None
    summary = {'type': 'summary', 'comparison_arm': comparison, 'trials': len(results), 'arms': arms,
               'eligible_speed_pairs': len(eligible), 'valid_successes': {m: a['successes'] for m, a in arms.items()},
               'normal_handoff_rate': sum(r['handoff_used'] for r in results if r['mode'] == 'normal') /
                    max(1, sum(r['mode'] == 'normal' for r in results)),
               'paired_direct_over_handoff_ratios': ratios, 'median_direct_over_handoff_ratio': ratio,
               'all_attempt_paired_ratios': all_ratios,
               'median_all_attempt_paired_ratio': statistics.median(all_ratios) if all_ratios else None,
               'target_met': bool(comparison == 'jev' and len(complete) >= 3 and len(eligible) >= 3 and ratio is not None and ratio >= 2 and all_ratios and statistics.median(all_ratios) >= 2
                                  and arms[comparison]['successes'] >= arms['direct']['successes']),
               'note': 'Success-only ratios exclude failures, which remain in all-attempt timing and counts. '
                       'Timeout timings are censored. No inferred model calls, token totals, or prices.'}
    groups = {(r.get('phase', 'discovery'), r.get('task', 'navigation')) for r in results}
    if len(groups) > 1:
        summary['by_phase_task'] = {f'{phase}/{task}': summarize([r for r in results
            if r.get('phase', 'discovery') == phase and r.get('task', 'navigation') == task])
            for phase, task in sorted(groups)}
        summary['target_met'] = all(g['target_met'] for g in summary['by_phase_task'].values())
    summary['full_suite_target_met'] = bool(summary['target_met'] and groups == {
        (phase, task) for phase in ('discovery', 'heldout') for task in TASKS})
    return summary


def self_test():
    from urllib.request import Request, urlopen
    from urllib.parse import urlencode
    from urllib.error import HTTPError
    trace = extract_trace('\n'.join(json.dumps(e) for e in [
        {'type': 'text_delta', 'text': 'I used handoff'},
        {'type': 'tool_start', 'id': 'x', 'name': 'browser'},
        {'type': 'tool_input', 'delta': '{"action":'},
        {'type': 'tool_input', 'delta': '"handoff"}'},
        {'type': 'tool_exec', 'id': 'x', 'name': 'browser'},
        {'type': 'tool_done', 'id': 'x', 'name': 'browser', 'error': None,
         'output': '{"decision_provider":"jcode","status":"done","action_trace":[{"status":"executed"}]}'}]))
    assert trace['browser_calls'][0]['action'] == 'handoff'
    assert trace['browser_calls'][0]['executed']
    assert trace['browser_calls'][0]['decision_provider'] == 'jcode'
    assert trace['browser_calls'][0]['handoff_status'] == 'done'
    assert trace['browser_calls'][0]['handoff_executed_steps'] == 1
    assert handoff_metrics(trace)['handoff_effective']
    call = trace['browser_calls'][0]
    for status, steps, error, effective in [('hand_back', 0, None, False),
                                            ('hand_back', 2, None, True),
                                            ('done', 0, None, True),
                                            ('done', 2, 'failed', False),
                                            (None, 0, None, False)]:
        sample = {'browser_calls': [dict(call, handoff_status=status, handoff_executed_steps=steps, error=error)]}
        assert handoff_metrics(sample)['handoff_effective'] == effective
    assert not extract_trace('{"type":"text_delta","text":"handoff"}')['browser_calls']
    assert environment(Path('/isolated-runtime'))['JCODE_DEBUG_CONTROL'] == '1'
    assert summarize([])['median_direct_over_handoff_ratio'] is None
    normal = {'pair': 1, 'mode': 'normal', 'valid_success': True,
              'handoff_used': True, 'handoff_effective': True, 'elapsed_seconds': 2}
    direct = {'pair': 1, 'mode': 'direct', 'valid_success': True,
              'handoff_used': False, 'elapsed_seconds': 3}
    assert summarize([normal, direct])['median_direct_over_handoff_ratio'] == 1.5
    assert summarize([normal, dict(direct, valid_success=False)])['eligible_speed_pairs'] == 0
    assert summarize([dict(normal, handoff_used=False), direct])['eligible_speed_pairs'] == 0
    assert summarize([dict(normal, handoff_effective=False), direct])['eligible_speed_pairs'] == 0
    assert environment(Path('/isolated-runtime'), 'direct')['JCODE_BROWSER_HANDOFF_DISABLED'] == '1'
    assert environment(Path('/isolated-runtime'), 'jev')['JCODE_BROWSER_HANDOFF_DISABLED'] == '0'
    assert timing_fields({'action_trace': [{'duration_ms': 12}], 'timing': {'total_ms': 40}}) == {
        'action_trace[0].duration_ms': 12, 'timing': {'total_ms': 40}}
    telemetry = extract_trace('\n'.join(json.dumps(e) for e in [
        {'type': 'tokens', 'input': 23, 'output': 4},
        {'type': 'done', 'text': 'receipt', 'usage': {'input_tokens': 23},
         'model_call_count': 2, 'actual_cost_usd': 0.001}]))
    assert telemetry['model_call_count'] == 2 and telemetry['actual_cost_usd'] == 0.001
    assert telemetry['usage_tokens'] == {'input_tokens': 23} and len(telemetry['token_events']) == 1
    assert extract_trace('')['model_call_count'] is None
    assert extract_trace('')['actual_cost_usd'] is None
    failed = summarize([normal, dict(direct, valid_success=False, timed_out=True, handoff_used=True,
                                    protocol_compliant=False)])
    assert failed['arms']['direct']['timeouts'] == 1 and failed['arms']['direct']['handoff_violations'] == 1
    assert failed['all_attempt_paired_ratios'] == [1.5] and failed['eligible_speed_pairs'] == 0
    target = [dict(r, pair=i, mode='jev' if r['mode'] == 'normal' else 'direct',
                   elapsed_seconds=1 if r['mode'] == 'normal' else 3)
              for i in range(3) for r in (normal, direct)]
    assert summarize(target)['target_met']
    assert not summarize(target)['full_suite_target_met']
    full = [dict(r, task=task, phase=phase) for task in TASKS
            for phase in ('discovery', 'heldout') for r in target]
    assert summarize(full)['full_suite_target_met']
    assert not summarize(target[:4])['target_met']
    assert not summarize([dict(r, valid_success=False) if r['mode'] == 'jev' else r for r in target])['target_met']
    fixture = Fixture()
    try:
        token, url = fixture.add()
        assert not fixture.state(token)['receipt_page_served']
        root = url.rsplit('/', 1)[0]
        for step in ('start', 'guides', 'browser', 'navigation'):
            with urlopen(root + '/' + step) as response:
                assert response.status == 200
        with urlopen(root + '/receipt') as response:
            assert fixture.state(token)['receipt'].encode() in response.read()
        assert fixture.state(token)['receipt_page_served']
        assert not fixture.state(token)['confirmation_dom_observed']
        with urlopen(Request(root + '/visible', data=b'')) as response:
            assert response.status == 200
        assert fixture.state(token)['confirmation_dom_observed']
        with urlopen(Request(root + '/left', data=b'')) as response:
            assert response.status == 200
        assert not fixture.state(token)['confirmation_dom_observed']
        assert server_outcome(fixture.state(token))
        # Exercise all task variants, required route order, bad fields, and hidden receipts.
        for phase in ('discovery', 'heldout'):
            for task in TASKS:
                token, url = fixture.add(task, phase)
                root = url.rsplit('/', 1)[0]
                values = TASKS[task][phase]
                def get(step):
                    with urlopen(root + '/' + step) as response:
                        return response.read().decode()
                def post(step, fields):
                    with urlopen(Request(root + '/' + step, data=urlencode(fields).encode())) as response:
                        return response.read().decode()
                if task != 'navigation':
                    try:
                        get('receipt')
                        raise AssertionError('Premature receipt served')
                    except HTTPError as exc:
                        assert exc.code == 404
                get('start')
                if task == 'navigation':
                    assert values[0] in get('guides')
                    assert values[1] in get('browser')
                    get('navigation')
                elif task == 'search':
                    assert 'No matching results' in get('results?q=wrong&category=all')
                    assert not fixture.state(token)['search_valid']
                    get('results?' + urlencode({'q': values[0], 'category': values[1]}))
                    get('item')
                    get('navigation')
                else:
                    get('request')
                    fields = dict(zip(('name', 'email', 'notes'), values))
                    assert 'Invalid' in post('submit', fields)
                    assert not fixture.state(token)['form_valid']
                    fields.update(delivery='pickup', confirm='yes')
                    assert 'View navigation receipt' in post('submit', fields)
                get('receipt')
                state = fixture.state(token)
                assert server_outcome(state)
                assert not server_outcome(dict(state, requests=[]))
                assert not state['confirmation_dom_observed']
                post('visible', {})
                assert fixture.state(token)['confirmation_dom_observed']
    finally:
        fixture.close()
    print(json.dumps({'type': 'self_test', 'passed': True, 'live_browser_or_jcode_launched': False}))


_TAB_LOCK = None


def bridge_call(action, tab_id, **params):
    bridge = Path(os.environ['JCODE_HOME']) / 'browser/browser'
    probe = subprocess.run([str(bridge), action, json.dumps(dict(params, tabId=tab_id))],
                           capture_output=True, text=True, timeout=20, check=True)
    return json.loads(probe.stdout)


def guard_tab(tab_id):
    """Read-only preflight, invoked only for explicitly requested live runs."""
    global _TAB_LOCK
    runtime = Path(os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}'))
    fd = os.open(runtime / f'jcode-browser-acceptance-tab-{tab_id}.lock',
                 os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    _TAB_LOCK = os.fdopen(fd, 'w')
    fcntl.flock(_TAB_LOCK, fcntl.LOCK_EX | fcntl.LOCK_NB)
    listing = bridge_call('listTabs', tab_id)
    tab = next((tab for window in listing.get('windows', []) for tab in window.get('tabs', [])
                if tab.get('tabId') == tab_id), None)
    if not tab:
        raise RuntimeError('Designated disposable tab not found')
    url = urlsplit(tab['url'])
    if not (url.scheme in ('http', 'https') and url.hostname in ('127.0.0.1', 'localhost', '::1')
            and tab.get('title') in ('Jcode isolated browser fixture', 'Jev hybrid verified')):
        raise RuntimeError('Refusing non-loopback/non-fixture tab. Prepare a dedicated fixture tab first.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', help='Exact newly built binary, not a mutable launcher')
    parser.add_argument('--model')
    parser.add_argument('--jcode-home', type=Path, help='Caller-prepared isolated Jcode home with required auth and browser bridge')
    parser.add_argument('--provider')
    parser.add_argument('--expected-handoff-provider', choices=('jcode', 'openrouter'), default='jcode')
    parser.add_argument('--tab-id', type=int, help='Coordinator-owned disposable tab, reused serially')
    parser.add_argument('--trials', type=int, default=3, help='Pairs per task per phase (at least 3 for acceptance)')
    parser.add_argument('--tasks', nargs='+', choices=tuple(TASKS), default=list(TASKS))
    parser.add_argument('--phase', choices=('discovery', 'heldout', 'all'), default='all')
    parser.add_argument('--arm', choices=('jev', 'normal'), default='jev', help='Explicit Jev or separate natural-default experiment')
    parser.add_argument('--timeout', type=float, default=240)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.binary or not args.model or args.tab_id is None or args.output is None or args.jcode_home is None:
        parser.error('--binary, --model, --tab-id, --jcode-home, and --output are required for live runs')
    if not os.environ.get('BROWSER_SESSION', '').strip():
        parser.error('An existing BROWSER_SESSION matching the disposable tab is required')
    if args.tab_id <= 0:
        parser.error('--tab-id must be positive')
    if args.trials < 1 or args.timeout <= 0:
        parser.error('--trials and --timeout must be positive')
    isolated_home = args.jcode_home.resolve(strict=True)
    if not isolated_home.is_dir() or isolated_home == (Path.home() / '.jcode').resolve():
        parser.error('--jcode-home must be a prepared isolated directory, not ~/.jcode')
    os.environ['JCODE_HOME'] = str(isolated_home)
    guard_tab(args.tab_id)
    args.binary = str(Path(args.binary).resolve(strict=True))
    args.output = args.output.resolve()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    metadata = {'binary': args.binary, 'binary_sha256': hashlib.sha256(Path(args.binary).read_bytes()).hexdigest(),
                'model': args.model, 'provider': args.provider, 'tab_id': args.tab_id,
                'pairs_per_task_phase': args.trials, 'tasks': args.tasks, 'phase': args.phase, 'arm': args.arm, 'expected_handoff_provider': args.expected_handoff_provider, 'timing': 'run process start through process exit, daemon startup excluded'}
    (args.output / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
    # Short private runtime path avoids Unix socket path limits. Do not alter shared daemon.
    with tempfile.TemporaryDirectory(prefix='jbh-', dir=os.environ.get('JCODE_SCRATCH_DIR', '/tmp')) as directory:
        runtime = Path(directory)
        fixture = Fixture()
        results = []
        try:
            with (args.output / 'metrics.ndjson').open('w') as metrics:
                pair = 0
                phases = ('discovery', 'heldout') if args.phase == 'all' else (args.phase,)
                for phase in phases:
                    for task in args.tasks:
                        for repetition in range(args.trials):
                            pair += 1
                            for mode in ((args.arm, 'direct') if pair % 2 else ('direct', args.arm)):
                                trial_runtime = runtime / f'{pair}-{mode}'
                                trial_runtime.mkdir()
                                server = None
                                started = time.monotonic()
                                try:
                                    with (args.output / f'{pair:02d}-{mode}-server.log').open('w') as log:
                                        server = start_server(args.binary, trial_runtime / 'server.sock',
                                                              environment(trial_runtime, mode), log)
                                    result = run_trial(args, fixture, trial_runtime, args.output, pair, mode, task, phase)
                                except Exception as exc:
                                    result = {'type': 'trial', 'pair': pair, 'mode': mode, 'task': task, 'phase': phase,
                                              'valid_success': False, 'handoff_used': False, 'handoff_effective': False,
                                              'protocol_compliant': False, 'timed_out': isinstance(exc, subprocess.TimeoutExpired),
                                              'elapsed_seconds': time.monotonic() - started, 'infrastructure_error': str(exc),
                                              'timing_scope': 'infrastructure failure including daemon startup'}
                                finally:
                                    if server is not None:
                                        terminate(server)
                                results.append(result)
                                line = json.dumps(result)
                                print(line, flush=True)
                                metrics.write(line + '\n')
                                metrics.flush()
                summary = summarize(results)
                print(json.dumps(summary), flush=True)
                metrics.write(json.dumps(summary) + '\n')
                (args.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        finally:
            fixture.close()


if __name__ == '__main__':
    main()
