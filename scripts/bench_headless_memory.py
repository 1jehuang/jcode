#!/usr/bin/env python3
"""Headless multi-turn memory benchmark: jcode vs Claude Code.

Swarm workers run headless, so this measures what matters for swarms: total
PSS of every process involved after N concurrent headless sessions have each
completed K real model turns (including tool calls).

jcode:  one isolated `jcode serve` daemon, N headless sessions created over the
        debug socket, turns sent with `message:` commands.
Claude: N persistent `claude -p --input-format stream-json` processes, turns
        sent as stream-json user messages.

PSS is summed over each tool's full process tree (server/children/MCP, etc).
Auth and config are borrowed read-only from the real homes. Prompts are
read-only so the benchmark never modifies the working directory.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench_memory_cli import sum_tree_pss, terminate_pgroup, wait_for_socket  # noqa: E402

TURNS = [
    "Use a tool to list the files in the current directory, then reply with just the number of entries.",
    "Read the first 20 lines of README.md with a tool and reply with its top-level title only.",
    "Search the repository for the string 'fn main' with a tool and reply with how many files matched.",
    "In one sentence, summarize what you have learned about this repository so far.",
    "Reply with exactly the word: done",
]


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# jcode
# ---------------------------------------------------------------------------

def debug_cmd(sock_path: str, cmd: str, session_id: str | None = None, timeout: float = 600) -> str:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    sock.connect(sock_path)
    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id
    sock.sendall((json.dumps(req) + "\n").encode())
    data = b""
    deadline = time.time() + timeout
    while time.time() < deadline:
        chunk = sock.recv(1 << 20)
        if not chunk:
            break
        data += chunk
        # Responses are newline-delimited; skip non-final events.
        while b"\n" in data:
            line, data = data.split(b"\n", 1)
            if not line.strip():
                continue
            resp = json.loads(line)
            if resp.get("type") in (None, "debug_response") or "ok" in resp:
                sock.close()
                if not resp.get("ok", False):
                    raise RuntimeError(f"{cmd[:40]}: {resp.get('error') or resp.get('output')}")
                return resp.get("output", "")
    sock.close()
    raise TimeoutError(cmd[:40])


def run_jcode(sessions: int, turns: int, cwd: str, model: str, memory: bool) -> dict:
    jcode = shutil.which("jcode") or str(Path.home() / ".local/bin/jcode")
    temp_root = tempfile.mkdtemp(prefix="jcode-headless-bench-")
    home = Path(temp_root) / "home"
    run = Path(temp_root) / "run"
    home.mkdir()
    run.mkdir()
    real = Path.home() / ".jcode"
    for name in ("auth.json", "anthropic-auth.json", "openai-auth.json", "config.toml", "models"):
        if (real / name).exists():
            (home / name).symlink_to(real / name)
    env = os.environ.copy()
    env.update(
        {
            "JCODE_HOME": str(home),
            "JCODE_RUNTIME_DIR": str(run),
            "JCODE_TEMP_SERVER": "1",
            "JCODE_SERVER_OWNER_PID": str(os.getpid()),
            "JCODE_NO_TELEMETRY": "1",
            "JCODE_DEBUG_CONTROL": "1",
            "JCODE_MEMORY_ENABLED": "1" if memory else "0",
            "JCODE_EMBEDDING_IDLE_UNLOAD_SECS": "86400",
        }
    )
    main_sock = str(run / "bench.sock")
    dbg_sock = str(run / "bench-debug.sock")
    server = subprocess.Popen(
        [jcode, "--no-update", "--no-selfdev", "serve", "--socket", main_sock],
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=open(Path(temp_root) / "server.log", "w"),
        preexec_fn=os.setsid,
    )
    pgid = os.getpgid(server.pid)
    samples = []
    try:
        if not wait_for_socket(dbg_sock, 30):
            raise RuntimeError("jcode debug socket not ready")
        ids = []
        for _ in range(sessions):
            out = debug_cmd(dbg_sock, f"create_session:{cwd}", timeout=120)
            sid = json.loads(out)["session_id"]
            if model:
                debug_cmd(dbg_sock, f"set_model:{model}", sid, timeout=60)
            ids.append(sid)
        if memory:
            # Builds compiled with the optional local embedding stack keep the
            # shared model resident for the whole run (worst case). Default
            # builds recall memories remotely, so this is a recorded no-op.
            load = json.loads(debug_cmd(dbg_sock, "embeddings:load", timeout=120))
            embeddings_status = load.get("status") if load.get("status") == "loaded" else load.get("error")
        else:
            embeddings_status = "memory disabled"
        time.sleep(1.0)
        samples.append(("idle", *sum_tree_pss([server.pid], [pgid])))
        log(f"jcode sessions={sessions} idle pss={samples[-1][1]} MB")
        errors: list[str] = []
        for t in range(turns):
            prompt = TURNS[t % len(TURNS)]

            def one(sid: str) -> None:
                try:
                    debug_cmd(dbg_sock, f"message:{prompt}", sid, timeout=600)
                except Exception as exc:  # noqa: BLE001
                    errors.append(f"turn {t + 1} {sid[:16]}: {exc}")

            threads = [threading.Thread(target=one, args=(sid,)) for sid in ids]
            for th in threads:
                th.start()
            for th in threads:
                th.join()
            time.sleep(1.0)
            samples.append((f"turn{t + 1}", *sum_tree_pss([server.pid], [pgid])))
            log(f"jcode sessions={sessions} turn {t + 1} pss={samples[-1][1]} MB")
        version = subprocess.run([jcode, "--version"], capture_output=True, text=True).stdout.strip()
        checks = {"embeddings": embeddings_status}
        try:
            hist = json.loads(debug_cmd(dbg_sock, "history", ids[0], timeout=30))
            checks["history_messages"] = len(hist) if isinstance(hist, list) else None
            blob = json.dumps(hist)
            checks["tool_uses"] = blob.count('"tool_use"') + blob.count('"ToolUse"') + blob.count('"tool_calls"')
            checks["last_response"] = debug_cmd(dbg_sock, "last_response", ids[0], timeout=30)[:200]
        except Exception as exc:  # noqa: BLE001
            checks["error"] = str(exc)[:200]
        return {"tool": "jcode" + ("" if memory else " (memory off)"), "sessions": sessions,
                "samples": samples, "errors": errors, "version": version, "checks": checks}
    finally:
        terminate_pgroup(pgid)
        shutil.rmtree(temp_root, ignore_errors=True)


# ---------------------------------------------------------------------------
# Claude Code
# ---------------------------------------------------------------------------

class ClaudeSession:
    def __init__(self, claude: str, cwd: str, model: str):
        argv = [claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json",
                "--verbose", "--permission-mode", "bypassPermissions"]
        if model:
            argv += ["--model", model]
        self.proc = subprocess.Popen(
            argv, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, preexec_fn=os.setsid, text=True, bufsize=1,
        )
        self.pgid = os.getpgid(self.proc.pid)
        self.tool_uses = 0
        self.last_result = ""

    def turn(self, prompt: str, timeout: float = 600) -> str | None:
        msg = {"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": prompt}]}}
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = self.proc.stdout.readline()
            if not line:
                return "claude exited"
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if ev.get("type") == "assistant":
                self.tool_uses += json.dumps(ev).count('"tool_use"')
            if ev.get("type") == "result":
                self.last_result = str(ev.get("result"))[:200]
                return None if not ev.get("is_error") else str(ev.get("result"))[:200]
        return "timeout"


def run_claude(sessions: int, turns: int, cwd: str, model: str) -> dict:
    claude = shutil.which("claude") or str(Path.home() / ".local/bin/claude")
    procs = [ClaudeSession(claude, cwd, model) for _ in range(sessions)]
    samples = []
    errors: list[str] = []
    try:
        time.sleep(3.0)
        roots = [p.proc.pid for p in procs]
        pgids = [p.pgid for p in procs]
        samples.append(("idle", *sum_tree_pss(roots, pgids)))
        log(f"claude sessions={sessions} idle pss={samples[-1][1]} MB")
        for t in range(turns):
            prompt = TURNS[t % len(TURNS)]

            def one(p: ClaudeSession) -> None:
                err = p.turn(prompt)
                if err:
                    errors.append(f"turn {t + 1}: {err}")

            threads = [threading.Thread(target=one, args=(p,)) for p in procs]
            for th in threads:
                th.start()
            for th in threads:
                th.join()
            time.sleep(1.0)
            samples.append((f"turn{t + 1}", *sum_tree_pss(roots, pgids)))
            log(f"claude sessions={sessions} turn {t + 1} pss={samples[-1][1]} MB")
        version = subprocess.run([claude, "--version"], capture_output=True, text=True).stdout.strip()
        return {"tool": "Claude Code", "sessions": sessions, "samples": samples,
                "errors": errors, "version": version,
                "checks": {"tool_uses": procs[0].tool_uses, "last_response": procs[0].last_result}}
    finally:
        for p in procs:
            try:
                p.proc.stdin.close()
            except Exception:
                pass
            terminate_pgroup(p.pgid)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--sessions", type=int, nargs="+", default=[1, 5, 10, 20])
    ap.add_argument("--turns", type=int, default=5)
    ap.add_argument("--tools", nargs="+", default=["jcode", "claude"])
    ap.add_argument("--jcode-model", default="claude-sonnet-4-6")
    ap.add_argument("--claude-model", default="claude-sonnet-4-6")
    ap.add_argument("--cwd", default=os.getcwd())
    ap.add_argument("--json-out")
    args = ap.parse_args()

    results = []
    for n in args.sessions:
        for tool in args.tools:
            if tool == "jcode":
                results.append(run_jcode(n, args.turns, args.cwd, args.jcode_model, memory=True))
            elif tool == "jcode_memory_off":
                results.append(run_jcode(n, args.turns, args.cwd, args.jcode_model, memory=False))
            elif tool == "claude":
                results.append(run_claude(n, args.turns, args.cwd, args.claude_model))
            print(json.dumps(results[-1]), flush=True)
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(results, indent=2))
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal.default_int_handler)
    raise SystemExit(main())
