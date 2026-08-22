#!/usr/bin/env python3
"""Mock sponsor service and CLI for end-to-end referral attribution tests.

This models the boundary a real sponsor owns: a CLI starts signup with a
referrer, a signed magic link crosses a browser round trip, and the service
persists an immutable acquisition source on first account creation.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import sqlite3
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

MAX_SOURCE_LENGTH = 100
TOKEN_TTL_SECONDS = 300


def encode_token(payload: dict[str, Any], secret: bytes) -> str:
    body = base64.urlsafe_b64encode(json.dumps(payload, separators=(",", ":")).encode()).rstrip(b"=")
    signature = hmac.new(secret, body, hashlib.sha256).digest()
    return f"{body.decode()}.{base64.urlsafe_b64encode(signature).decode().rstrip('=')}"


def decode_token(token: str, secret: bytes, now: int | None = None) -> dict[str, Any]:
    try:
        body_text, signature_text = token.split(".", 1)
        body = body_text.encode()
        signature = base64.urlsafe_b64decode(signature_text + "=" * (-len(signature_text) % 4))
        expected = hmac.new(secret, body, hashlib.sha256).digest()
        if not hmac.compare_digest(signature, expected):
            raise ValueError("invalid token signature")
        payload = json.loads(base64.urlsafe_b64decode(body + b"=" * (-len(body) % 4)))
    except (ValueError, TypeError, json.JSONDecodeError) as error:
        raise ValueError("invalid signup token") from error
    current = int(time.time()) if now is None else now
    if int(payload.get("exp", 0)) < current:
        raise ValueError("expired signup token")
    return payload


def connect_db(path: Path) -> sqlite3.Connection:
    db = sqlite3.connect(path)
    db.execute(
        "CREATE TABLE IF NOT EXISTS accounts (email TEXT PRIMARY KEY, acquisition_source TEXT, created_at INTEGER NOT NULL)"
    )
    db.execute("CREATE TABLE IF NOT EXISTS consumed_tokens (nonce TEXT PRIMARY KEY, consumed_at INTEGER NOT NULL)")
    db.commit()
    return db


def json_request(url: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.loads(response.read())


def make_handler(db_path: Path, secret: bytes):
    class Handler(BaseHTTPRequestHandler):
        def send_json(self, status: int, payload: dict[str, Any]) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def read_json(self) -> dict[str, Any]:
            return json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))

        def do_POST(self) -> None:  # noqa: N802
            if self.path != "/v1/signup":
                self.send_json(404, {"error": "not found"})
                return
            try:
                request = self.read_json()
                email = str(request["email"]).strip().lower()
                raw_source = request.get("via")
                source = str(raw_source).strip() if raw_source is not None else None
                source = source or None
                if "@" not in email:
                    raise ValueError("invalid email")
                if source is not None and (len(source) > MAX_SOURCE_LENGTH or not source.isascii()):
                    raise ValueError("invalid attribution source")
                nonce = hashlib.sha256(f"{email}:{time.time_ns()}".encode()).hexdigest()
                token = encode_token(
                    {"email": email, "via": source, "nonce": nonce, "exp": int(time.time()) + TOKEN_TTL_SECONDS},
                    secret,
                )
                self.send_json(200, {"magic_link": f"{server_url(self.server)}/v1/confirm?token={token}"})
            except (KeyError, ValueError, json.JSONDecodeError) as error:
                self.send_json(400, {"error": str(error)})

        def do_GET(self) -> None:  # noqa: N802
            parsed = urllib.parse.urlsplit(self.path)
            if parsed.path == "/":
                self.send_json(200, {"product": "mock-sponsor"})
                return
            if parsed.path == "/health":
                self.send_json(200, {"ok": True})
                return
            if parsed.path == "/v1/discovery":
                query = urllib.parse.parse_qs(parsed.query)
                listing = {
                    "name": "mock-sponsor",
                    "blurb": "Reference virtual payment service for attribution validation",
                    "url": f"{server_url(self.server)}/?via=jcode-discovery",
                }
                if query.get("tool", [None])[0] == "mock-sponsor":
                    listing["setup"] = (
                        f"Run `python {Path(__file__).resolve()} signup --service "
                        f"{server_url(self.server)} --email <email> --via jcode-discovery`, "
                        "then confirm the returned magic link."
                    )
                    self.send_json(200, {"tool": listing})
                else:
                    self.send_json(200, {"tools": [listing]})
                return
            if parsed.path == "/v1/confirm":
                try:
                    token = urllib.parse.parse_qs(parsed.query)["token"][0]
                    payload = decode_token(token, secret)
                    with connect_db(db_path) as db:
                        db.execute("BEGIN IMMEDIATE")
                        if db.execute("SELECT 1 FROM consumed_tokens WHERE nonce = ?", (payload["nonce"],)).fetchone():
                            raise ValueError("signup token already consumed")
                        db.execute(
                            "INSERT OR IGNORE INTO accounts(email, acquisition_source, created_at) VALUES (?, ?, ?)",
                            (payload["email"], payload.get("via"), int(time.time())),
                        )
                        db.execute(
                            "INSERT INTO consumed_tokens(nonce, consumed_at) VALUES (?, ?)",
                            (payload["nonce"], int(time.time())),
                        )
                    self.send_json(200, {"email": payload["email"], "confirmed": True})
                except (KeyError, IndexError, ValueError, sqlite3.IntegrityError) as error:
                    self.send_json(400, {"error": str(error)})
                return
            if parsed.path == "/v1/account":
                email = urllib.parse.parse_qs(parsed.query).get("email", [""])[0].lower()
                with connect_db(db_path) as db:
                    row = db.execute(
                        "SELECT email, acquisition_source, created_at FROM accounts WHERE email = ?", (email,)
                    ).fetchone()
                if not row:
                    self.send_json(404, {"error": "account not found"})
                else:
                    self.send_json(200, {"email": row[0], "acquisition_source": row[1], "created_at": row[2]})
                return
            self.send_json(404, {"error": "not found"})

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


def server_url(server: ThreadingHTTPServer) -> str:
    host, port = server.server_address[:2]
    return f"http://{host}:{port}"


def run_server(args: argparse.Namespace) -> None:
    server = ThreadingHTTPServer((args.host, args.port), make_handler(args.db, args.secret.encode()))
    print(server_url(server), flush=True)
    server.serve_forever()


def cli_signup(args: argparse.Namespace) -> None:
    result = json_request(f"{args.service}/v1/signup", {"email": args.email, "via": args.via})
    print(result["magic_link"])


def cli_confirm(args: argparse.Namespace) -> None:
    print(json.dumps(json_request(args.magic_link), sort_keys=True))


def cli_account(args: argparse.Namespace) -> None:
    query = urllib.parse.urlencode({"email": args.email})
    print(json.dumps(json_request(f"{args.service}/v1/account?{query}"), sort_keys=True))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(required=True)
    serve = sub.add_parser("serve")
    serve.add_argument("--host", default="127.0.0.1")
    serve.add_argument("--port", type=int, default=0)
    serve.add_argument("--db", type=Path, required=True)
    serve.add_argument("--secret", default="mock-sponsor-development-secret")
    serve.set_defaults(func=run_server)
    signup = sub.add_parser("signup")
    signup.add_argument("--service", required=True)
    signup.add_argument("--email", required=True)
    signup.add_argument("--via")
    signup.set_defaults(func=cli_signup)
    confirm = sub.add_parser("confirm")
    confirm.add_argument("magic_link")
    confirm.set_defaults(func=cli_confirm)
    account = sub.add_parser("account")
    account.add_argument("--service", required=True)
    account.add_argument("--email", required=True)
    account.set_defaults(func=cli_account)
    args = parser.parse_args()
    return args


if __name__ == "__main__":
    parsed = parse_args()
    try:
        parsed.func(parsed)
    except urllib.error.HTTPError as error:
        raise SystemExit(error.read().decode()) from error
