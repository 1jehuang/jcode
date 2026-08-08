#!/usr/bin/env python3
"""Live-turn probe against a *real* jcode gateway.

`protocol_smoke_test.py` asserts the scripted mock's event sequence. This asserts
the real server: it opens the real WebSocket, subscribes, sends a real prompt,
and requires an actual model turn to stream back and complete.

Reuses the websocket framing from protocol_smoke_test so there is one
implementation of the wire handling.

Exit 0 when the turn streams and (optionally) contains the expected text.
"""
import argparse
import json
import sys
import time

from protocol_smoke_test import ws_connect, ws_recv, ws_send

# A live provider turn is slower than the mock's scripted one, and cold provider
# init (auth refresh, model resolution) happens on the first message.
DEFAULT_TIMEOUT_S = 180.0

# Request ids used on the wire; `done` events echo the id they complete.
SUBSCRIBE_ID = 1
MESSAGE_ID = 2


def drain(sock, deadline, on_event):
    """Read newline-delimited events until on_event returns True or we time out."""
    sock.settimeout(2.0)
    while time.time() < deadline:
        try:
            opcode, data = ws_recv(sock)
        except TimeoutError:
            continue
        except OSError:
            return False
        if opcode == 0x9:  # ping
            continue
        if opcode == 0x8:  # close
            return False
        for line in data.decode("utf-8", errors="replace").split("\n"):
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if on_event(event):
                return True
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--token", required=True)
    ap.add_argument(
        "--working-dir",
        default="",
        help="Absolute dir to subscribe against. Defaults to whatever /health advertises, "
        "matching what the app does with the value from /pair.",
    )
    ap.add_argument("--prompt", default="Reply with exactly: REAL_GATEWAY_OK")
    ap.add_argument("--expect", default="")
    ap.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_S)
    ap.add_argument("--json-out", default="")
    args = ap.parse_args()

    failures = []
    seen_types = []
    raw_events = []
    text = []

    def check(name, cond, detail=""):
        print(f"  [{'PASS' if cond else 'FAIL'}] {name}{f' ({detail})' if detail else ''}")
        if not cond:
            failures.append(name)

    working_dir = args.working_dir
    if not working_dir:
        import http.client

        conn = http.client.HTTPConnection(args.host, args.port, timeout=5)
        conn.request("GET", "/health")
        health = json.loads(conn.getresponse().read().decode())
        conn.close()
        working_dir = health.get("working_dir") or ""
    check("server advertises a working_dir", bool(working_dir), working_dir)

    sock = ws_connect(args.host, args.port, args.token)
    check("websocket upgrade accepted by real gateway", True)

    # The real server sends a snapshot/connected preamble; subscribing is what
    # binds this client to a session, exactly as the app does.
    subscribe = {"id": SUBSCRIBE_ID, "type": "subscribe"}
    if working_dir:
        subscribe["working_dir"] = working_dir
    ws_send(sock, json.dumps(subscribe) + "\n")
    deadline = time.time() + min(args.timeout, 30.0)

    session_id = [None]

    def watch_preamble(event):
        etype = event.get("type", "")
        seen_types.append(etype)
        raw_events.append(event)
        if event.get("session_id"):
            session_id[0] = event["session_id"]
        # Any of these prove the real server accepted the subscription.
        # `state` is the server's post-subscribe snapshot (ServerEvent::State).
        return etype in ("state", "session_id", "history")

    got_preamble = drain(sock, deadline, watch_preamble)
    check(
        "real server responded to subscribe",
        got_preamble or bool(seen_types),
        f"types={sorted(set(seen_types))[:6]}",
    )

    # Real model turn.
    ws_send(
        sock,
        json.dumps({"id": MESSAGE_ID, "type": "message", "content": args.prompt}) + "\n",
    )
    deadline = time.time() + args.timeout
    saw_delta = [False]
    saw_done = [False]
    error = [None]

    def watch_turn(event):
        etype = event.get("type", "")
        seen_types.append(etype)
        raw_events.append(event)
        # Names come straight from ServerEvent in crates/jcode-protocol/src/wire.rs.
        if etype == "text_delta":
            saw_delta[0] = True
            text.append(event.get("text") or "")
        elif etype == "text_replace":
            saw_delta[0] = True
            text[:] = [event.get("text") or ""]
        elif etype == "error":
            error[0] = event.get("message") or json.dumps(event)
            return True
        elif etype == "done":
            # Only the message's own id ends the turn; a stray subscribe `done`
            # must not be mistaken for it.
            if event.get("id") in (None, MESSAGE_ID):
                saw_done[0] = True
                return True
        return False

    drain(sock, deadline, watch_turn)
    body = "".join(text)

    check("real model streamed output", saw_delta[0], f"{len(body)} chars")
    check("turn completed", saw_done[0])
    check("no server error", error[0] is None, error[0] or "")
    if args.expect:
        check(f"output contains {args.expect!r}", args.expect in body, body.strip()[:120])

    if args.json_out:
        with open(args.json_out, "w") as fh:
            json.dump(
                {
                    "session_id": session_id[0],
                    "event_types": sorted(set(seen_types)),
                    "events": raw_events[:200],
                    "text": body,
                    "failures": failures,
                },
                fh,
                indent=1,
            )

    if body.strip():
        print(f"  model said: {body.strip()[:200]!r}")
    if failures:
        print(f"FAILED: {', '.join(failures)}")
        return 1
    print("LIVE TURN OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
