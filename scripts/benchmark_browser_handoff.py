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
                    state['requests'].append({'step': step, 'method': self.command})
                    root = '/' + token
                    body = None
                    pages = {
                        'start': ('Jcode documentation', 'guides', 'Browse guides'),
                        'guides': ('Guides', 'browser', 'Browser guide'),
                        'browser': ('Browser guide', 'navigation', 'Navigation examples'),
                        'navigation': ('Navigation examples', 'receipt', 'View navigation receipt'),
                    }
                    if not post and step in pages:
                        title, target, label = pages[step]
                        body = f'<h1>{title}</h1><a href="{root}/{target}">{label}</a>'
                    elif not post and step == 'receipt':
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

    def add(self):
        token = secrets.token_hex(12)
        with self.lock:
            self.trials[token] = {'receipt': secrets.token_hex(5), 'requests': [],
                                  'receipt_page_served': False, 'confirmation_dom_observed': False}
        return token, f'http://127.0.0.1:{self.server.server_port}/{token}/start'

    def state(self, token):
        with self.lock:
            return json.loads(json.dumps(self.trials[token]))

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def extract_trace(text):
    """Use actual tool input/exec/done events, never assistant prose mentions."""
    calls, current, final, errors = {}, None, '', []
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if not isinstance(event, dict):
            continue
        kind = event.get('type')
        if kind == 'tool_start':
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
    return {'browser_calls': browser, 'other_tools': other, 'final_text': final, 'errors': errors}


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


def environment(runtime):
    env = dict(os.environ)
    # Retain normal credentials/config, but do not inherit another agent's routing.
    for key in ('JCODE_SOCKET', 'JCODE_SESSION_ID', 'JCODE_PARENT_SESSION_ID'):
        env.pop(key, None)
    env['JCODE_RUNTIME_DIR'] = str(runtime)
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


def run_trial(args, fixture, runtime, output, index, mode):
    token, url = fixture.add()
    trial_dir = output / f'{index:02d}-{mode}'
    trial_dir.mkdir()
    workspace = trial_dir / 'workspace'
    workspace.mkdir()
    prompt = (f'Use the browser in tab {args.tab_id} to visit {url}. Navigate through the guides '
              'to the Browser guide, then Navigation examples, then view the navigation receipt. '
              'Tell me the receipt displayed on the final page and leave that page open. '
              'Use only the browser tool for this task. Stay in this tab.')
    if mode == 'direct':
        prompt += ' Do not use browser action="handoff". Complete the task using direct browser actions only.'
    (trial_dir / 'prompt.txt').write_text(prompt)
    socket = runtime / 'server.sock'
    env = environment(runtime)
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
    correct = state['receipt_page_served'] and state['confirmation_dom_observed'] and final_dom_correct
    receipt_correct = state['receipt'] in trace.pop('final_text')
    providers = [c.get('decision_provider') for c in trace['browser_calls']
                 if c.get('executed') and c['action'] == 'handoff']
    provider_valid = all(p == args.expected_handoff_provider for p in providers)
    compliant = provider_valid and not trace['other_tools'] and not unknown and (mode != 'direct' or not handoff)
    result = {'type': 'trial', 'pair': index, 'mode': mode, 'model': args.model,
              'elapsed_seconds': round(elapsed, 3), 'exit_code': process.returncode,
              'timed_out': timed_out, 'handoff_used': handoff, **handoff_metrics(trace),
              'decision_providers': providers, 'expected_handoff_provider': args.expected_handoff_provider, 'actions': actions,
              'trace_complete': not unknown and bool(actions), 'correct_page_state': correct,
              'final_dom_correct': final_dom_correct, 'validation_error': validation_error,
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
        pairs.setdefault(r['pair'], {})[r['mode']] = r
    eligible = [p for p in pairs.values() if len(p) == 2 and all(r['valid_success'] for r in p.values())
                and p['normal']['handoff_used'] and p['normal'].get('handoff_effective', False)
                and not p['direct']['handoff_used']]
    ratios = [p['direct']['elapsed_seconds'] / p['normal']['elapsed_seconds'] for p in eligible]
    return {'type': 'summary', 'trials': len(results), 'eligible_speed_pairs': len(eligible),
            'normal_handoff_rate': sum(r['handoff_used'] for r in results if r['mode'] == 'normal') /
                max(1, sum(r['mode'] == 'normal' for r in results)),
            'valid_successes': {mode: sum(r['valid_success'] for r in results if r['mode'] == mode)
                                for mode in ('normal', 'direct')},
            'paired_direct_over_handoff_ratios': ratios,
            'median_direct_over_handoff_ratio': statistics.median(ratios) if ratios else None,
            'note': 'Ratio >1 favors handoff. Observed paired timings, not a guaranteed speedup. '
                    'Only correct compliant pairs with a done or progress-making normal-arm handoff are speed-eligible.'}


def self_test():
    from urllib.request import Request, urlopen
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
    parser.add_argument('--trials', type=int, default=3, help='Number of paired trials')
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
                'paired_trials': args.trials, 'expected_handoff_provider': args.expected_handoff_provider, 'timing': 'run process start through process exit, daemon startup excluded'}
    (args.output / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
    # Short private runtime path avoids Unix socket path limits. Do not alter shared daemon.
    with tempfile.TemporaryDirectory(prefix='jbh-', dir=os.environ.get('JCODE_SCRATCH_DIR', '/tmp')) as directory:
        runtime = Path(directory)
        fixture = Fixture()
        results = []
        try:
            with (args.output / 'metrics.ndjson').open('w') as metrics:
                for pair in range(1, args.trials + 1):
                    for mode in (('normal', 'direct') if pair % 2 else ('direct', 'normal')):
                        trial_runtime = runtime / f'{pair}-{mode}'
                        trial_runtime.mkdir()
                        with (args.output / f'{pair:02d}-{mode}-server.log').open('w') as log:
                            server = start_server(args.binary, trial_runtime / 'server.sock',
                                                  environment(trial_runtime), log)
                        try:
                            result = run_trial(args, fixture, trial_runtime, args.output, pair, mode)
                        finally:
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
