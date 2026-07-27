#!/usr/bin/env python3
"""End-to-end check that a jcode gateway on another host is usable.

Verifies the full remote path: /health -> /pair -> WebSocket -> subscribe ->
a real agent turn on the remote machine.

    # on the remote host
    jcode pair

    # here
    python3 scripts/remote/remote_check.py --host <ip> --code 123456 \
        --working-dir 'C:\\Users\\you'

Once paired, the token is cached and --code is no longer needed.
"""
import argparse, json, os, pathlib, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gateway_client import Gateway, health, pair

TOKEN_DIR = pathlib.Path.home() / ".jcode" / "remote-tokens"


def token_path(host, port):
    return TOKEN_DIR / f"{host}_{port}.json"


def load_or_pair(host, port, code, device_id, device_name):
    path = token_path(host, port)
    if path.exists() and not code:
        return json.loads(path.read_text())["token"]
    if not code:
        sys.exit(f"No cached token for {host}:{port}. Run `jcode pair` on the "
                 f"remote host and pass --code.")
    result = pair(host, port, code, device_id, device_name)
    TOKEN_DIR.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(result))
    path.chmod(0o600)
    return result["token"]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--host", required=True)
    p.add_argument("--port", type=int, default=7643)
    p.add_argument("--code", help="pairing code from `jcode pair` on the remote host")
    p.add_argument("--working-dir", required=True)
    p.add_argument("--device-id", default="jcode-remote-check")
    p.add_argument("--device-name", default="jcode remote check")
    p.add_argument("--prompt", default="Run a shell command that prints the OS "
                                       "name and current directory, then report the result.")
    p.add_argument("--timeout", type=float, default=150.0)
    args = p.parse_args()

    failures = []

    def check(label, ok, detail=""):
        print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" - {detail}" if detail else ""))
        if not ok:
            failures.append(label)

    print(f"jcode remote check -> {args.host}:{args.port}\n")

    info = health(args.host, args.port)
    check("reachable /health", info.get("status") == "ok", str(info))
    check("gateway enabled", info.get("gateway") is True,
          "" if info.get("gateway") is True else
          "set [gateway] enabled = true and restart the remote server")

    token = load_or_pair(args.host, args.port, args.code,
                         args.device_id, args.device_name)
    check("paired (token available)", bool(token))

    t0 = time.time()
    g = Gateway(args.host, args.port, token)
    check("websocket upgrade", True, f"{(time.time() - t0) * 1000:.0f}ms")

    sub_id = g.send(type="subscribe", working_dir=args.working_dir)
    subscribed = False
    for ev in g.events(time.time() + 20):
        if ev.get("type") == "done" and ev.get("id") == sub_id:
            subscribed = True
            break
    check("subscribe acknowledged", subscribed)

    # History is not pushed on subscribe; it must be requested explicitly.
    hist_id = g.send(type="get_history")
    history = None
    for ev in g.events(time.time() + 20):
        if ev.get("type") == "history":
            history = ev
            break
        if ev.get("type") == "done" and ev.get("id") == hist_id:
            break
    check("get_history -> history", history is not None,
          "" if history is None else
          f"{len(history.get('messages') or [])} msgs, "
          f"{len(history.get('available_models') or [])} models")

    t0 = time.time()
    g.send(type="message", content=args.prompt)
    text, tools, saw_done = "", [], False
    for ev in g.events(time.time() + args.timeout):
        kind = ev.get("type")
        if kind == "text_delta":
            text += ev.get("text", "")
        elif kind == "tool_done":
            tools.append(ev.get("name"))
        elif kind == "error":
            check("no error event", False, json.dumps(ev)[:200])
        elif kind == "done":
            saw_done = True
            break
    check("agent turn completed", saw_done, f"{time.time() - t0:.1f}s")
    check("assistant produced text", bool(text.strip()))
    if tools:
        print(f"  [info] remote tools executed: {', '.join(t for t in tools if t)}")
    g.close()

    print(f"\n--- remote reply ---\n{text.strip()[:600]}\n")
    if failures:
        print(f"FAILED: {len(failures)} check(s): {', '.join(failures)}")
        return 1
    print("ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
