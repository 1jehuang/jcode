#!/usr/bin/env python3
"""Tiny OpenAI-compatible chat server for cloud-move e2e tests.

Each request is appended to $FAKE_LOG as JSON (the full message list the
agent sent), and the reply echoes a short marker so tests can prove which
machine produced which turn and what context the model saw.
"""
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = os.environ["FAKE_LOG"]
LABEL = os.environ.get("FAKE_LABEL", "model")


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        body = json.dumps({"data": [{"id": "fake-model", "object": "model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        msgs = req.get("messages", [])
        with open(LOG, "a") as f:
            f.write(json.dumps({"label": LABEL, "messages": msgs}) + "\n")
        flat = json.dumps(msgs)
        seen_notice = "moved from" in flat and "to cloud host" in flat
        text = f"[{LABEL}] ack. saw_cloud_notice={seen_notice}"
        if not req.get("stream"):
            body = json.dumps({
                "id": "x", "object": "chat.completion", "model": "fake-model",
                "choices": [{"index": 0, "finish_reason": "stop",
                             "message": {"role": "assistant", "content": text}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            }).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        def ev(obj):
            self.wfile.write(b"data: " + json.dumps(obj).encode() + b"\n\n")
            self.wfile.flush()
        ev({"id": "x", "object": "chat.completion.chunk", "model": "fake-model",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}}]})
        ev({"id": "x", "object": "chat.completion.chunk", "model": "fake-model",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}})
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


port = int(sys.argv[1])
HTTPServer(("127.0.0.1", port), H).serve_forever()
