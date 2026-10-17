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


# ---------------------------------------------------------------------------
# Codex CLI, OpenCode, pi
# ---------------------------------------------------------------------------
#
# These tools are authenticated from jcode's own OpenAI OAuth account
# (~/.jcode/openai-auth.json) into a throwaway home, so the benchmark never
# touches the user's real ~/.codex, ~/.local/share/opencode or ~/.pi.


def resolve_bin(name: str, override: str | None) -> str:
    path = override or shutil.which(name)
    if not path:
        raise RuntimeError(f"{name} not found on PATH (pass --{name}-bin)")
    return path


def provision_openai_homes(root: Path, account: str | None) -> dict[str, dict[str, str]]:
    homes = {
        "codex": {"CODEX_HOME": str(root / "codex")},
        "opencode": {"XDG_DATA_HOME": str(root / "opencode"),
                     "XDG_CONFIG_HOME": str(root / "opencode-config"),
                     "XDG_STATE_HOME": str(root / "opencode-state")},
        "pi": {"PI_CODING_AGENT_DIR": str(root / "pi")},
    }
    if os.environ.get("OPENAI_API_KEY"):
        # API-key mode: every tool reads OPENAI_API_KEY from the environment.
        (root / "codex").mkdir(parents=True, exist_ok=True)
        key_auth = root / "codex" / "auth.json"
        key_auth.write_text(json.dumps({"auth_mode": "apikey", "OPENAI_API_KEY": os.environ["OPENAI_API_KEY"]}))
        key_auth.chmod(0o600)
        return homes
    data = json.loads((Path.home() / ".jcode" / "openai-auth.json").read_text())
    label = account or data.get("active_openai_account")
    acct = next(a for a in data["openai_accounts"] if a["label"] == label)

    def write(path: Path, obj: dict) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(obj))
        path.chmod(0o600)

    write(root / "codex" / "auth.json", {
        "auth_mode": "chatgpt", "OPENAI_API_KEY": None,
        "tokens": {"id_token": acct["id_token"], "access_token": acct["access_token"],
                   "refresh_token": acct["refresh_token"], "account_id": acct["account_id"]},
        "last_refresh": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    })
    oauth = {"type": "oauth", "access": acct["access_token"], "refresh": acct["refresh_token"],
             "expires": acct["expires_at"], "accountId": acct["account_id"]}
    write(root / "opencode" / "opencode" / "auth.json", {"openai": oauth})
    write(root / "pi" / "auth.json", {"openai-codex": oauth})
    return {
        "codex": {"CODEX_HOME": str(root / "codex")},
        "opencode": {"XDG_DATA_HOME": str(root / "opencode"),
                     "XDG_CONFIG_HOME": str(root / "opencode-config"),
                     "XDG_STATE_HOME": str(root / "opencode-state")},
        "pi": {"PI_CODING_AGENT_DIR": str(root / "pi")},
    }


class JsonLineProc:
    """A persistent child speaking newline-delimited JSON over stdio."""

    def __init__(self, argv: list[str], cwd: str, env: dict[str, str]):
        self.proc = subprocess.Popen(
            argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, preexec_fn=os.setsid, text=True, bufsize=1,
        )
        self.pgid = os.getpgid(self.proc.pid)
        self.tool_uses = 0
        self.last_result = ""

    def send(self, obj: dict) -> None:
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def read_until(self, done, timeout: float = 600) -> str | None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = self.proc.stdout.readline()
            if not line:
                return "process exited"
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            verdict = done(ev)
            if verdict is not None:
                return verdict or None
        return "timeout"

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        terminate_pgroup(self.pgid)


class CodexSession(JsonLineProc):
    """`codex app-server` JSON-RPC: one process per session, like Codex's own headless clients."""

    def __init__(self, codex: str, cwd: str, env: dict[str, str], model: str):
        super().__init__([codex, "app-server"], cwd, env)
        self.next_id = 1
        self.request("initialize", {"clientInfo": {"name": "jcode-bench", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "initialized"})
        res = self.request("thread/start", {"cwd": cwd, "model": model or None,
                                            "approvalPolicy": "never", "sandbox": "read-only"})
        self.thread_id = res["thread"]["id"]

    def request(self, method: str, params: dict) -> dict:
        rid = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        out: dict = {}

        def done(ev: dict):
            if ev.get("id") == rid and "method" not in ev:
                if "error" in ev:
                    raise RuntimeError(f"codex {method}: {ev['error']}")
                out.update(ev.get("result") or {})
                return ""
            return None

        err = self.read_until(done, timeout=120)
        if err:
            raise RuntimeError(f"codex {method}: {err}")
        return out

    def turn(self, prompt: str, timeout: float = 600) -> str | None:
        rid = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": rid, "method": "turn/start", "params": {
            "threadId": self.thread_id, "input": [{"type": "text", "text": prompt}]}})

        def done(ev: dict):
            method = ev.get("method", "")
            if "id" in ev and method:
                # Server-initiated request (approval etc): decline so the turn continues.
                self.send({"jsonrpc": "2.0", "id": ev["id"], "result": {"decision": "decline"}})
                return None
            if ev.get("id") == rid and "error" in ev:
                return str(ev["error"])[:200]
            if method == "item/completed":
                item = (ev.get("params") or {}).get("item") or {}
                kind = item.get("type", "")
                if kind in ("commandExecution", "mcpToolCall", "fileChange", "webSearch"):
                    self.tool_uses += 1
                if kind == "agentMessage":
                    self.last_result = str(item.get("text"))[:200]
            if method == "turn/completed":
                turn = (ev.get("params") or {}).get("turn") or {}
                return "" if turn.get("status") in (None, "completed") else str(turn.get("error"))[:200]
            return None

        return self.read_until(done, timeout)


class PiSession(JsonLineProc):
    """`pi --mode rpc`: one process per session."""

    def __init__(self, pi: str, cwd: str, env: dict[str, str], model: str):
        provider = "openai" if os.environ.get("OPENAI_API_KEY") else "openai-codex"
        argv = [pi, "--mode", "rpc", "--no-session", "--provider", provider]
        if model:
            argv += ["--model", model]
        super().__init__(argv, cwd, env)

    def turn(self, prompt: str, timeout: float = 600) -> str | None:
        self.send({"type": "prompt", "message": prompt})

        def done(ev: dict):
            kind = ev.get("type")
            if kind == "response" and ev.get("success") is False:
                return str(ev.get("error"))[:200]
            if kind == "tool_execution_start":
                self.tool_uses += 1
            if kind == "agent_end":
                msgs = ev.get("messages") or []
                last = msgs[-1] if msgs else {}
                self.last_result = json.dumps(last)[:200]
                if last.get("stopReason") == "error" or last.get("errorMessage"):
                    return str(last.get("errorMessage") or "pi turn failed")[:200]
                return ""
            return None

        return self.read_until(done, timeout)


def run_stdio_tool(name: str, factory, sessions: int, turns: int) -> dict:
    procs = []
    samples = []
    errors: list[str] = []
    try:
        procs = [factory() for _ in range(sessions)]
        time.sleep(3.0)
        roots = [p.proc.pid for p in procs]
        pgids = [p.pgid for p in procs]
        samples.append(("idle", *sum_tree_pss(roots, pgids)))
        log(f"{name} sessions={sessions} idle pss={samples[-1][1]} MB")
        for t in range(turns):
            prompt = TURNS[t % len(TURNS)]

            def one(p) -> None:
                try:
                    err = p.turn(prompt)
                except Exception as exc:  # noqa: BLE001
                    err = str(exc)[:200]
                if err:
                    errors.append(f"turn {t + 1}: {err}")

            threads = [threading.Thread(target=one, args=(p,)) for p in procs]
            for th in threads:
                th.start()
            for th in threads:
                th.join()
            time.sleep(1.0)
            samples.append((f"turn{t + 1}", *sum_tree_pss(roots, pgids)))
            log(f"{name} sessions={sessions} turn {t + 1} pss={samples[-1][1]} MB")
        return {"tool": name, "sessions": sessions, "samples": samples, "errors": errors,
                "checks": {"tool_uses": procs[0].tool_uses, "last_response": procs[0].last_result}}
    finally:
        for p in procs:
            p.close()


def run_codex(sessions: int, turns: int, cwd: str, model: str, env: dict[str, str], binary: str) -> dict:
    result = run_stdio_tool("Codex CLI", lambda: CodexSession(binary, cwd, env, model), sessions, turns)
    result["version"] = subprocess.run([binary, "--version"], capture_output=True, text=True, env=env).stdout.strip()
    return result


def run_pi(sessions: int, turns: int, cwd: str, model: str, env: dict[str, str], binary: str) -> dict:
    result = run_stdio_tool("pi", lambda: PiSession(binary, cwd, env, model), sessions, turns)
    result["version"] = subprocess.run([binary, "--version"], capture_output=True, text=True, env=env).stdout.strip()
    return result


def http_json(method: str, url: str, body: dict | None = None, timeout: float = 600):
    import urllib.request

    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read()
    return json.loads(raw) if raw else None


def run_opencode(sessions: int, turns: int, cwd: str, model: str, env: dict[str, str], binary: str) -> dict:
    """One `opencode serve` server hosting N sessions, the same shape as jcode's daemon."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    server = subprocess.Popen(
        [binary, "serve", "--port", str(port), "--hostname", "127.0.0.1"], cwd=cwd, env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, preexec_fn=os.setsid,
    )
    pgid = os.getpgid(server.pid)
    samples = []
    errors: list[str] = []
    tool_uses = 0
    last = ""
    try:
        deadline = time.time() + 60
        while True:
            try:
                http_json("GET", f"{base}/session", timeout=5)
                break
            except Exception:
                if time.time() > deadline:
                    raise RuntimeError("opencode server not ready")
                time.sleep(0.25)
        ids = [http_json("POST", f"{base}/session", {})["id"] for _ in range(sessions)]
        time.sleep(1.0)
        samples.append(("idle", *sum_tree_pss([server.pid], [pgid])))
        log(f"opencode sessions={sessions} idle pss={samples[-1][1]} MB")
        model_ref = {"providerID": "openai", "modelID": model} if model else None
        for t in range(turns):
            prompt = TURNS[t % len(TURNS)]
            replies: dict[str, dict] = {}

            def one(sid: str) -> None:
                body = {"parts": [{"type": "text", "text": prompt}]}
                if model_ref:
                    body["model"] = model_ref
                try:
                    reply = http_json("POST", f"{base}/session/{sid}/message", body)
                    replies[sid] = reply or {}
                    err = (reply or {}).get("info", {}).get("error")
                    if err:
                        errors.append(f"turn {t + 1}: {str(err)[:200]}")
                except Exception as exc:  # noqa: BLE001
                    errors.append(f"turn {t + 1}: {exc}")

            threads = [threading.Thread(target=one, args=(sid,)) for sid in ids]
            for th in threads:
                th.start()
            for th in threads:
                th.join()
            first = replies.get(ids[0], {})
            parts = first.get("parts") or []
            tool_uses += sum(1 for p in parts if p.get("type") == "tool")
            texts = [p.get("text", "") for p in parts if p.get("type") == "text"]
            last = (texts[-1] if texts else last)[:200]
            time.sleep(1.0)
            samples.append((f"turn{t + 1}", *sum_tree_pss([server.pid], [pgid])))
            log(f"opencode sessions={sessions} turn {t + 1} pss={samples[-1][1]} MB")
        version = subprocess.run([binary, "--version"], capture_output=True, text=True, env=env).stdout.strip()
        return {"tool": "OpenCode", "sessions": sessions, "samples": samples, "errors": errors,
                "version": version, "checks": {"tool_uses": tool_uses, "last_response": last}}
    finally:
        terminate_pgroup(pgid)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--sessions", type=int, nargs="+", default=[1, 5, 10, 20])
    ap.add_argument("--turns", type=int, default=5)
    ap.add_argument("--tools", nargs="+", default=["jcode", "claude"])
    ap.add_argument("--jcode-model", default="claude-sonnet-4-6")
    ap.add_argument("--claude-model", default="claude-sonnet-4-6")
    ap.add_argument("--openai-model", default="gpt-5.4",
                    help="model for codex, opencode and pi (all use ChatGPT OAuth)")
    ap.add_argument("--openai-account", help="jcode OpenAI account label to borrow (default: active)")
    ap.add_argument("--codex-bin")
    ap.add_argument("--opencode-bin")
    ap.add_argument("--pi-bin")
    ap.add_argument("--cwd", default=os.getcwd())
    ap.add_argument("--json-out")
    args = ap.parse_args()

    results = []
    auth_root = Path(tempfile.mkdtemp(prefix="headless-bench-auth-"))
    homes: dict[str, dict[str, str]] = {}
    if {"codex", "opencode", "pi"} & set(args.tools):
        homes = provision_openai_homes(auth_root, args.openai_account)
    try:
        return run_all(args, results, homes)
    finally:
        shutil.rmtree(auth_root, ignore_errors=True)


def run_all(args, results: list, homes: dict[str, dict[str, str]]) -> int:
    for n in args.sessions:
        for tool in args.tools:
            if tool == "jcode":
                results.append(run_jcode(n, args.turns, args.cwd, args.jcode_model, memory=True))
            elif tool == "jcode_memory_off":
                results.append(run_jcode(n, args.turns, args.cwd, args.jcode_model, memory=False))
            elif tool == "claude":
                results.append(run_claude(n, args.turns, args.cwd, args.claude_model))
            elif tool in ("codex", "opencode", "pi"):
                runner = {"codex": run_codex, "opencode": run_opencode, "pi": run_pi}[tool]
                binary = resolve_bin(tool, getattr(args, f"{tool}_bin"))
                env = dict(os.environ, **homes[tool])
                results.append(runner(n, args.turns, args.cwd, args.openai_model, env, binary))
            else:
                raise SystemExit(f"unknown tool: {tool}")
            print(json.dumps(results[-1]), flush=True)
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(results, indent=2))
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGINT, signal.default_int_handler)
    raise SystemExit(main())
