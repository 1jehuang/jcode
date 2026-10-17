#!/usr/bin/env python3
"""Token-protected HTTP service that generates jcode pairing codes.

GET /pair-code?t=<token> -> {"code": "123456", "host": "100.109.78.41", "port": 7643, "uri": "jcode://pair?..."}
"""
import http.server, json, re, subprocess, os
from urllib.parse import urlparse, parse_qs

TOKEN = open('/etc/jcode-pair-token').read().strip()
HOST = '100.109.78.41'
PORT = 7643

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if q.get('t', [''])[0] != TOKEN:
            return self._send(403, {'error': 'forbidden'})
        if u.path != '/pair-code':
            return self._send(404, {'error': 'not found'})
        env = dict(os.environ)
        env['PATH'] = '/home/ec2-user/.local/bin:' + env.get('PATH', '')
        env['KCODE_GATEWAY_HOST'] = HOST
        try:
            out = subprocess.run(
                ['sudo', '-u', 'ec2-user', '-i', 'jcode', 'pair'],
                capture_output=True, text=True, timeout=30, env=env,
            )
            text = re.sub(r'\x1b\[[0-9;]*m', '', out.stdout + out.stderr)
            m = re.search(r'Pairing code:\s+(\d{3})\s(\d{3})', text)
            if not m:
                return self._send(500, {'error': 'no code in output'})
            code = m.group(1) + m.group(2)
            uri = f'jcode://pair?host={HOST}&port={PORT}&code={code}'
            return self._send(200, {'code': code, 'host': HOST, 'port': PORT, 'uri': uri, 'expires_in': 300})
        except Exception as e:
            return self._send(500, {'error': str(e)})

http.server.HTTPServer(('0.0.0.0', 7644), H).serve_forever()
