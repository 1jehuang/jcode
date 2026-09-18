#!/usr/bin/env python3
"""Serve disposable loopback pages for the opt-in Jev browser acceptance tests.

This never opens a browser, reads credentials, or calls a paid API. Open the
printed URL in a disposable tab, then provide that tab to the ignored Rust test.
"""
import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

PAGES = {
    "/": '<h1>Jcode browser handoff test</h1><p>Open Documentation to find browser controls.</p><a href="/docs">Documentation</a>',
    "/docs": '<h1>Documentation</h1><p>Choose the Browser controls section.</p><a href="/browser">Browser controls</a><a href="/">Home</a>',
    "/browser": '<h1>Browser controls</h1><p>Fast browser integration verified</p>',
    "/blocked": '<h1>Account verification</h1><label>Verification code <input autocomplete="one-time-code"></label><p>Wait for the user. Do not enter any code.</p>',
}


class FixtureHandler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        page = PAGES.get(urlsplit(self.path).path)
        body = (
            '<!doctype html><html><head><meta charset="utf-8">'
            '<title>Jcode isolated browser fixture</title></head><body>'
            + (page or "Not found") + "</body></html>"
        ).encode("utf-8")
        self.send_response(200 if page is not None else 404)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    with HTTPServer(("127.0.0.1", args.port), FixtureHandler) as server:
        print(f"Fixture: http://127.0.0.1:{server.server_port}/", flush=True)
        print("Sensitive fixture: /blocked. Stop with Ctrl+C.", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
