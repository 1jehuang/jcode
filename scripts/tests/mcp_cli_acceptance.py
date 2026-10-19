#!/usr/bin/env python3
"""Stdlib-only public MCP CLI acceptance replay. All mutable state stays in scratch.

python3 cli_acceptance.py --binary /absolute/path/jcode --output /scratch/result.json
No CLI output, credentials, or authorization URLs are persisted in the report.
"""
import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import re
import secrets
import stat
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request

DEFAULT_SCRATCH_ROOT = Path(os.environ.get('JCODE_SCRATCH_DIR', str(Path.home() / '.jcode' / 'scratch')))


def require(condition, message):
    if not condition:
        raise AssertionError(message)


class Fixture:
    def __init__(self):
        self.events = []
        self.errors = []
        self.lock = threading.Lock()
        self.token = secrets.token_urlsafe(32)
        self.static = secrets.token_urlsafe(32)
        self.code = secrets.token_urlsafe(24)
        self.registration = None
        self.authorization = None
        self.streams = {}
        self.stop = threading.Event()
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *args):
                pass

            def reply(self, status, value=None, headers=None):
                event = getattr(self, 'boundary_event', None)
                if event is not None:
                    event['status'] = status
                body = b'' if value is None else json.dumps(value).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                for key, val in (headers or {}).items():
                    self.send_header(key, val)
                self.end_headers()
                if body:
                    self.wfile.write(body)

            def dispatch(self):
                path = urllib.parse.urlsplit(self.path).path
                raw = self.rfile.read(int(self.headers.get('Content-Length', '0')))
                data = {}
                if raw:
                    if 'application/json' in self.headers.get('Content-Type', ''):
                        data = json.loads(raw)
                    else:
                        data = {k: v[0] for k, v in urllib.parse.parse_qs(raw.decode()).items()}
                auth = self.headers.get('Authorization')
                auth_kind = ('oauth' if auth == 'Bearer ' + fixture.token else
                             'static' if auth == 'Bearer ' + fixture.static else
                             'absent' if auth is None else 'unexpected')
                event = {'verb': self.command, 'path': path,
                         'rpc': data.get('method'), 'auth': auth_kind,
                         'session': self.headers.get('Mcp-Session-Id'),
                         'protocol': self.headers.get('Mcp-Protocol-Version'),
                         'accept': self.headers.get('Accept')}
                self.boundary_event = event
                with fixture.lock:
                    fixture.events.append(event)
                base = fixture.base
                if path == '/resource-metadata':
                    self.reply(200, {'resource': base + '/mcp', 'authorization_servers': [base],
                                     'scopes_supported': ['tools:read']})
                elif path == '/.well-known/oauth-authorization-server':
                    self.reply(200, {'issuer': base, 'authorization_endpoint': base + '/authorize',
                                    'token_endpoint': base + '/token', 'registration_endpoint': base + '/register',
                                    'response_types_supported': ['code'], 'grant_types_supported': ['authorization_code'],
                                    'code_challenge_methods_supported': ['S256'],
                                    'token_endpoint_auth_methods_supported': ['none'],
                                    'authorization_response_iss_parameter_supported': True})
                elif path == '/register':
                    require(data.get('token_endpoint_auth_method') == 'none', 'DCR must request public client')
                    redirect = urllib.parse.urlsplit(data['redirect_uris'][0])
                    require(redirect.hostname == '127.0.0.1' and redirect.path == '/callback', 'DCR callback must be loopback')
                    fixture.registration = data
                    self.reply(201, {'client_id': 'fixture-client', 'token_endpoint_auth_method': 'none'})
                elif path == '/authorize':
                    pairs = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query)
                    require(all(len(v) == 1 for v in pairs.values()), 'duplicate authorize parameters')
                    q = {k: v[0] for k, v in pairs.items()}
                    require(q.get('response_type') == 'code' and q.get('client_id') == 'fixture-client', 'authorize client/response mismatch')
                    require(q.get('code_challenge_method') == 'S256', 'PKCE S256 required')
                    require(q.get('resource') == base + '/mcp' and q.get('scope') == 'tools:read', 'authorize resource/scope mismatch')
                    require(q.get('redirect_uri') in fixture.registration['redirect_uris'], 'unregistered callback')
                    fixture.authorization = q
                    self.reply(302, headers={'Location': q['redirect_uri'] + '?' + urllib.parse.urlencode(
                        {'code': fixture.code, 'state': q['state'], 'iss': base})})
                elif path == '/token':
                    q = fixture.authorization
                    require(q is not None, 'token before authorize')
                    challenge = base64.urlsafe_b64encode(hashlib.sha256(data.get('code_verifier', '').encode()).digest()).rstrip(b'=').decode()
                    require(challenge == q['code_challenge'], 'PKCE verifier mismatch')
                    require(data.get('grant_type') == 'authorization_code' and data.get('code') == fixture.code,
                            'token grant/code mismatch')
                    require(data.get('redirect_uri') == q['redirect_uri'] and data.get('client_id') == 'fixture-client', 'token callback/client mismatch')
                    require(data.get('resource') == base + '/mcp', 'token resource mismatch')
                    self.reply(200, {'access_token': fixture.token, 'token_type': 'Bearer', 'expires_in': 3600,
                                     'refresh_token': secrets.token_urlsafe(32), 'scope': 'tools:read'})
                elif path in ('/mcp', '/legacy', '/fallback', '/messages/legacy', '/messages/fallback'):
                    if auth_kind not in ('oauth', 'static'):
                        self.reply(401, headers={'WWW-Authenticate': 'Bearer resource_metadata="' + base + '/resource-metadata", scope="tools:read"'})
                        return
                    if path == '/fallback' and self.command == 'POST':
                        self.reply(405)
                        return
                    if path in ('/legacy', '/fallback') and self.command == 'GET':
                        stream = queue.Queue()
                        fixture.streams[path] = stream
                        event['status'] = 200
                        self.send_response(200)
                        self.send_header('Content-Type', 'text/event-stream')
                        self.send_header('Cache-Control', 'no-cache')
                        self.end_headers()
                        self.wfile.write(('event: endpoint\ndata: /messages' + path + '\n\n').encode())
                        self.wfile.flush()
                        while not fixture.stop.is_set():
                            try:
                                message = stream.get(timeout=0.2)
                            except queue.Empty:
                                continue
                            self.wfile.write(('event: message\ndata: ' + json.dumps(message) + '\n\n').encode())
                            self.wfile.flush()
                        self.close_connection = True
                        return
                    if self.command == 'DELETE':
                        self.reply(204)
                        return
                    if self.command == 'GET':
                        self.reply(405)
                        return
                    method = data.get('method')
                    if method == 'initialize':
                        result = {'protocolVersion': data['params']['protocolVersion'], 'capabilities': {'tools': {}},
                                  'serverInfo': {'name': 'acceptance-fixture', 'version': '1'}}
                    elif method == 'tools/list':
                        result = {'tools': [{'name': name, 'description': 'fixture tool', 'inputSchema': {'type': 'object'}}
                                            for name in ('zeta_fixture', 'alpha_fixture')]}
                    elif method == 'notifications/initialized':
                        self.reply(202)
                        return
                    else:
                        self.reply(400)
                        return
                    response = {'jsonrpc': '2.0', 'id': data['id'], 'result': result}
                    if path.startswith('/messages/'):
                        fixture.streams[path.removeprefix('/messages')].put(response)
                        self.reply(202)
                    else:
                        self.reply(200, response, {'Mcp-Session-Id': 'fixture-session'} if method == 'initialize' else {})
                else:
                    self.reply(404)

            def safe_dispatch(self):
                try:
                    self.dispatch()
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as exc:
                    fixture.errors.append(type(exc).__name__ + (': ' + str(exc) if isinstance(exc, AssertionError) else ''))
                    self.reply(500)

            do_GET = safe_dispatch
            do_POST = safe_dispatch
            do_DELETE = safe_dispatch

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.base = 'http://127.0.0.1:' + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.stop.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


class Replay:
    def __init__(self, binary, root, fixture):
        self.binary = str(binary)
        self.root = root
        self.fixture = fixture
        self.home = root / 'home'
        self.jhome = root / 'jcode-home'
        self.bin = root / 'browser-guards'
        for path in (self.home, self.jhome, self.bin):
            path.mkdir(mode=0o700)
        # Deliberately whitelist environment rather than inherit API keys/MCP credentials.
        self.env = {'HOME': str(self.home), 'JCODE_HOME': str(self.jhome),
                    'XDG_CONFIG_HOME': str(root / 'xdg-config'), 'XDG_CACHE_HOME': str(root / 'xdg-cache'),
                    'XDG_DATA_HOME': str(root / 'xdg-data'), 'XDG_RUNTIME_DIR': str(root / 'runtime'),
                    'PATH': str(self.bin) + ':/usr/bin:/bin', 'LANG': 'C.UTF-8', 'TERM': 'dumb',
                    'JCODE_DISABLE_CLAUDE_MCP': '1', 'NO_PROXY': '127.0.0.1,localhost'}
        (root / 'runtime').mkdir(mode=0o700)
        self.browser_marker = root / 'browser-invoked'
        for name in ('xdg-open', 'gio', 'sensible-browser', 'firefox', 'chromium', 'google-chrome'):
            p = self.bin / name
            p.write_text('#!/bin/sh\nprintf called >> "' + str(self.browser_marker) + '"\nexit 91\n')
            p.chmod(0o700)
        self.env['BROWSER'] = str(self.bin / 'xdg-open')
        self.config({'remote': {'type': 'http', 'url': fixture.base + '/mcp', 'enabled': False}})

    def config(self, servers):
        (self.jhome / 'mcp.json').write_text(json.dumps({'servers': servers}))

    def command(self, *args, success=True, timeout=12):
        proc = subprocess.run([self.binary, 'mcp', *args], cwd=self.root, env=self.env,
                              capture_output=True, text=True, timeout=timeout)
        require((proc.returncode == 0) == success, 'CLI exit status mismatch: ' + str(proc.returncode))
        require(not self.browser_marker.exists(), 'CLI attempted browser launch')
        return proc.stdout + proc.stderr

    def login(self):
        p = subprocess.Popen([self.binary, 'mcp', 'login', 'remote', '--no-browser'], cwd=self.root,
                             env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        lines = queue.Queue()
        def drain(pipe):
            for line in pipe:
                lines.put(line)
        threads = [threading.Thread(target=drain, args=(pipe,), daemon=True) for pipe in (p.stdout, p.stderr)]
        for t in threads:
            t.start()
        deadline = time.monotonic() + 15
        followed = False
        try:
            while time.monotonic() < deadline:
                try:
                    line = lines.get(timeout=0.1)
                except queue.Empty:
                    if p.poll() is not None:
                        break
                    continue
                match = re.search(r'http://127\.0\.0\.1:\d+/authorize\?\S+', line)
                if match:
                    url = match.group(0)
                    require(url.startswith(self.fixture.base + '/authorize?'), 'authorization endpoint not fixture')
                    # urllib follows the real HTTP 302 into the CLI's loopback callback.
                    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(url, timeout=5) as response:
                        require(response.status == 200, 'callback rejected')
                    followed = True
                    break
            require(followed, 'CLI did not emit authorization URL before deadline')
            require(p.wait(timeout=10) == 0, 'login CLI failed')
            for t in threads:
                t.join(timeout=1)
            require(not self.browser_marker.exists(), '--no-browser launched browser')
        finally:
            if p.poll() is None:
                p.kill()
                p.wait()
            p.stdout.close()
            p.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--scratch-root', type=Path, default=DEFAULT_SCRATCH_ROOT,
                        help='Scratch parent for isolated temporary runs and output (defaults to JCODE_SCRATCH_DIR or ~/.jcode/scratch)')
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    scratch_root = args.scratch_root.expanduser().resolve()
    require(output.is_relative_to(scratch_root), 'output must stay under scratch root')
    require(binary.is_file() and os.access(binary, os.X_OK), 'binary is not executable')
    scratch = scratch_root / 'mcp-acceptance-replay'
    scratch.mkdir(parents=True, exist_ok=True)
    report = {'binary': str(binary), 'cases': [], 'http_observations': [], 'fixture_errors': []}
    fixture = Fixture()
    with tempfile.TemporaryDirectory(prefix='run-', dir=scratch) as directory:
        replay = Replay(binary, Path(directory), fixture)
        def case(name, fn):
            start = time.monotonic()
            begin = len(fixture.events)
            try:
                fn()
                require(not fixture.errors, 'HTTP fixture rejected request')
                result = {'name': name, 'pass': True}
            except Exception as exc:
                # Assertion messages are fixed strings. Never capture CLI output or urllib URLs.
                result = {'name': name, 'pass': False,
                          'error': str(exc) if isinstance(exc, AssertionError) else type(exc).__name__}
            result['seconds'] = round(time.monotonic() - start, 4)
            result['http_begin'] = begin
            result['http_end'] = len(fixture.events)
            report['cases'].append(result)

        def listing():
            before = len(fixture.events)
            rows = json.loads(replay.command('list', '--json'))
            require([r['name'] for r in rows] == ['remote'], 'isolated list missing configured server')
            require(rows[0]['auth'] == 'no OAuth credentials stored', 'unexpected prelogin auth')
            require(len(fixture.events) == before, 'list performed network request')
        case('list_no_network', listing)

        def unauthenticated():
            before = len(fixture.events)
            start = time.monotonic()
            text = replay.command('test', 'remote', success=False, timeout=8)
            require('mcp login remote' in text, 'missing actionable login guidance')
            require(time.monotonic() - start < 8, 'unauthenticated test not fail-fast')
            events = fixture.events[before:]
            require(len(events) == 1 and events[0]['verb'] == 'POST'
                    and events[0]['path'] == '/mcp' and events[0]['rpc'] == 'initialize'
                    and events[0]['auth'] == 'absent' and events[0]['status'] == 401,
                    'unauthenticated test did not stop at the expected initialize 401')
        case('without_credentials_fail_fast_no_browser', unauthenticated)
        case('login_discovery_dcr_pkce_real_callback_no_browser', replay.login)

        store = replay.jhome / 'mcp-oauth.json'
        def private_store():
            require(store.exists(), 'OAuth credential store missing')
            require(stat.S_IMODE(store.stat().st_mode) == 0o600, 'credential file not mode 0600')
            require(stat.S_IMODE(replay.jhome.stat().st_mode) == 0o700, 'credential directory not mode 0700')
            require(fixture.token in store.read_text(), 'issued token not stored')
            before = len(fixture.events)
            rows = json.loads(replay.command('list', '--json'))
            require(rows[0]['auth'] == 'OAuth credentials stored', 'stored auth status missing')
            require(len(fixture.events) == before, 'postlogin list performed network')
        case('stored_token_private_mode_and_offline_status', private_store)

        def authenticated(name='remote', path='/mcp', kind='oauth', session=True):
            before = len(fixture.events)
            text = replay.command('test', name)
            require('disconnected cleanly' in text and 'Tools: 2' in text, 'test did not report clean shutdown/tools')
            require(text.index('alpha_fixture') < text.index('zeta_fixture'), 'CLI tools not sorted')
            events = fixture.events[before:]
            rpc = [e for e in events if e['rpc']]
            require([e['rpc'] for e in rpc] == ['initialize', 'notifications/initialized', 'tools/list'], 'MCP handshake order mismatch')
            require(all(e['auth'] == kind for e in events), 'authentication header missing or incorrect')
            if session:
                require(all(e['session'] == 'fixture-session' and e['protocol'] == '2025-06-18' for e in rpc[1:]),
                        'session/protocol headers not propagated')
                require(any(e['verb'] == 'DELETE' and e['session'] == 'fixture-session' for e in events), 'no session DELETE shutdown')
        case('authenticated_tools_headers_order_shutdown', authenticated)
        def logout():
            before = len(fixture.events)
            require('Removed local OAuth credentials' in replay.command('logout', 'remote'), 'logout did not remove credentials')
            rows = json.loads(replay.command('list', '--json'))
            require(rows[0]['auth'] == 'no OAuth credentials stored', 'logout left stored auth status')
            require(not store.exists() or fixture.token not in store.read_text(), 'logout retained issued access token')
            require(len(fixture.events) == before, 'logout/status performed network')
        case('logout', logout)
        case('after_logout_fail_fast_no_browser', unauthenticated)

        # Malformed store must never be consulted when static Authorization is set.
        replay.config({'static': {'type': 'http', 'url': fixture.base + '/mcp',
                                  'headers': {'aUtHoRiZaTiOn': 'Bearer ' + fixture.static}}})
        store.write_text('malformed-store-private-sentinel')
        def static_precedence():
            before = len(fixture.events)
            rows = json.loads(replay.command('list', '--json'))
            require(rows[0]['auth'] == 'static Authorization header', 'static list precedence failed')
            require(len(fixture.events) == before, 'static list performed network')
            authenticated('static', kind='static')
            require(store.read_text() == 'malformed-store-private-sentinel', 'static test altered OAuth store')
        case('static_authorization_over_malformed_oauth_store', static_precedence)
        for name, path, transport in [('legacy', '/legacy', 'sse'), ('fallback', '/fallback', 'http')]:
            replay.config({name: {'type': transport, 'url': fixture.base + path,
                                  'headers': {'Authorization': 'Bearer ' + fixture.static}}})
            def legacy_test(name=name, path=path):
                before = len(fixture.events)
                text = replay.command('test', name, timeout=15)
                require('disconnected cleanly' in text and 'Tools: 2' in text, 'legacy CLI test failed')
                events = fixture.events[before:]
                require(all(e['auth'] == 'static' for e in events), 'legacy static auth absent')
                require(any(e['verb'] == 'GET' and e['path'] == path for e in events), 'legacy SSE stream not opened')
                methods = [e['rpc'] for e in events if e['path'] == '/messages' + path]
                require(methods == ['initialize', 'notifications/initialized', 'tools/list'], 'legacy handshake order mismatch')
                if name == 'fallback':
                    require(events[0]['verb'] == 'POST' and events[0]['rpc'] == 'initialize', 'HTTP405 fallback did not probe HTTP first')
            case('explicit_legacy_sse' if name == 'legacy' else 'http405_legacy_sse_fallback', legacy_test)
    fixture.close()
    report['http_observations'] = fixture.events
    report['fixture_errors'] = fixture.errors
    report['pass'] = all(c['pass'] for c in report['cases']) and not fixture.errors
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'pass': report['pass'], 'cases': len(report['cases']), 'failed': [c['name'] for c in report['cases'] if not c['pass']], 'output': str(output)}))
    return 0 if report['pass'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
