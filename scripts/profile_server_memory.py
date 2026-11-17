#!/usr/bin/env python3
"""Profile one isolated headless server (Linux, standard library only).

Example:
  python3 scripts/profile_server_memory.py --binary ./target/selfdev/jcode \
      --cwd . --turns 5 --output /tmp/server-memory-run

Only the server PID is measured, never its process tree. Credentials are copied
into a disposable private home, never symlinked, and config is not borrowed.
Artifacts include raw smaps and smaps_rollup, build/version metadata and all
debug memory responses. Startup waits three seconds for asynchronous refreshes.
Add --heap-profile with a jemalloc-prof build to sample from process exec and
save a heap dump at each post-session stage (dumps themselves perturb memory).
Artifacts may contain model output and working-directory paths. No credentials
or environment dump are recorded. Real model turns may incur provider charges.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

AUTH_FILES = ("auth.json", "anthropic-auth.json", "openai-auth.json",
              "gemini_oauth.json", "antigravity_oauth.json")


HEAP_MALLOC_CONF = "prof:true,prof_active:true,prof_accum:true,lg_prof_sample:16"


def isolated_env(root: Path, inherited: dict[str, str], heap_profile: bool = False) -> dict[str, str]:
    env = {k: v for k, v in inherited.items() if not k.startswith("JCODE_")}
    env.update(JCODE_HOME=str(root / "home"), JCODE_RUNTIME_DIR=str(root / "run"),
               JCODE_TEST_SESSION="1", JCODE_TEMP_SERVER="1",
               JCODE_SERVER_OWNER_PID=str(os.getpid()), JCODE_NO_TELEMETRY="1",
               JCODE_DEBUG_CONTROL="1", TMPDIR=str(root / "tmp"),
               TMP=str(root / "tmp"), TEMP=str(root / "tmp"))
    if heap_profile:
        # Replace inherited settings so sampling starts at process exec.
        env["MALLOC_CONF"] = HEAP_MALLOC_CONF
        env["_RJEM_MALLOC_CONF"] = HEAP_MALLOC_CONF
    return env


def copy_auth(source: Path, home: Path) -> list[str]:
    copied = []
    for name in AUTH_FILES:
        src = source / name
        if src.is_file():
            # copyfile follows a source symlink but creates an independent destination.
            shutil.copyfile(src, home / name)
            (home / name).chmod(0o600)
            copied.append(name)
    return copied


def debug_cmd(path: Path, command: str, session_id: str | None, timeout: float) -> str:
    """bench_headless_memory.py's newline-delimited debug protocol."""
    request = {"type": "debug_command", "id": 1, "command": command}
    if session_id:
        request["session_id"] = session_id
    deadline = time.monotonic() + timeout
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(timeout)
        sock.connect(str(path))
        sock.sendall((json.dumps(request) + "\n").encode())
        data = b""
        while time.monotonic() < deadline:
            sock.settimeout(max(0.001, deadline - time.monotonic()))
            chunk = sock.recv(1 << 20)
            if not chunk:
                break
            data += chunk
            while b"\n" in data:
                line, data = data.split(b"\n", 1)
                if not line.strip():
                    continue
                response = json.loads(line)
                if response.get("type") in (None, "debug_response") or "ok" in response:
                    if not response.get("ok", False):
                        raise RuntimeError(f"debug command {command.split(':')[0]} failed: "
                                           f"{response.get('error') or response.get('output')}")
                    return response.get("output", "")
    raise RuntimeError(f"debug command {command.split(':')[0]} ended without a response")


def validate_population(info: dict, members: list, sid: str | None) -> None:
    expected = 1 if sid else 0
    if info.get("session_count") != expected or len(members) != expected:
        raise RuntimeError(f"expected {expected} live session(s) and swarm member(s), "
                           f"got {info.get('session_count')} and {len(members)}")
    if sid and (members[0].get("session_id") != sid or
                members[0].get("is_headless") is not True or
                members[0].get("live_attachments") != 0 or
                members[0].get("status") == "running"):
        raise RuntimeError("sole session must be the debug-created, idle headless session with no TUI")


def smaps(pid: int) -> dict:
    raw = Path(f"/proc/{pid}/smaps_rollup").read_text()
    kb = {}
    for line in raw.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[2] == "kB":
            kb[parts[0].rstrip(":")] = int(parts[1])
    if "Pss" not in kb or "Rss" not in kb:
        raise RuntimeError("smaps_rollup missing Pss/Rss")
    return {"pid": pid, "scope": "server_only", "raw": raw, "values_kib": kb}


def full_smaps(pid: int) -> dict:
    return {"pid": pid, "scope": "server_only",
            "raw": Path(f"/proc/{pid}/smaps").read_text()}


def save(path: Path, value) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def capture_pricing_cache(home: Path, output: Path) -> dict:
    path = home / "cache" / "models_dev_pricing.json"
    if not path.is_file():
        return {"available": False}
    cache = json.loads(path.read_text())
    providers = cache["providers"]
    save(output / "pricing_catalog.json", cache)
    return {"available": True, "providers": len(providers),
            "models": sum(len(models) for models in providers.values()),
            "cached_at_unix_secs": cache["cached_at_unix_secs"]}


def run(args) -> None:
    output = args.output.resolve()
    output.mkdir(mode=0o700)  # Refuse to overwrite a prior run.
    result = {"schema_version": 1, "binary": str(args.binary), "cwd": str(args.cwd),
              "turns": args.turns, "prompt": args.prompt, "model": args.model,
              "settle_seconds": args.settle_seconds,
              "startup_settle_seconds": args.startup_settle_seconds,
              "heap_profile": args.heap_profile,
              "require_pricing_refresh": args.require_pricing_refresh,
              "heap_malloc_conf": HEAP_MALLOC_CONF if args.heap_profile else None,
              "heap_dump_timing": "after stage memory snapshots, post-session stages only",
              "measurement_note": "debug memory reports and heap dumps allocate memory and may perturb later samples",
              "scope": "server_only",
              "config": "fresh defaults, no borrowed config", "stages": [], "status": "running"}
    save(output / "profile.json", result)
    try:
        with tempfile.TemporaryDirectory(prefix="runtime-", dir=output) as temp:
            root = Path(temp)
            for name in ("home", "run", "tmp"):
                (root / name).mkdir(mode=0o700)
            result["copied_auth_files"] = copy_auth(args.auth_from, root / "home")
            env = isolated_env(root, os.environ, args.heap_profile)
            result["isolation"] = {"private_home": True, "private_runtime": True,
                                   "private_tmpdir": True, "test_session": True}
            version = subprocess.run([str(args.binary), "--version"], env=env,
                                     cwd=args.cwd, capture_output=True, text=True,
                                     timeout=args.timeout, check=True)
            result["binary_version"] = version.stdout.strip()
            save(output / "profile.json", result)
            sock = root / "run" / "profile-debug.sock"
            with (output / "server.log").open("w") as log:
                server = subprocess.Popen(
                    [str(args.binary), "--no-update", "--no-selfdev", "serve", "--socket",
                     str(root / "run" / "profile.sock")], cwd=args.cwd, env=env,
                    stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
                result["server_pid"] = server.pid
                try:
                    deadline = time.monotonic() + args.timeout
                    while not sock.exists():
                        if server.poll() is not None:
                            raise RuntimeError(f"server exited {server.returncode}, see server.log")
                        if time.monotonic() >= deadline:
                            raise TimeoutError("private debug socket not ready, see server.log")
                        time.sleep(0.05)

                    def cmd(command, sid=None):
                        return debug_cmd(sock, command, sid, args.timeout)

                    def capture(stage, sid=None):
                        sample = {"stage": stage, "elapsed_seconds": time.monotonic() - started,
                                  "explicit_purge_completed": stage == "after_explicit_purge"}
                        result["stages"].append(stage)
                        # Persist each raw response immediately, including on later failure.
                        for key, command in (("server_info", "server:info"),
                                             ("members", "swarm:members")):
                            raw = cmd(command)
                            sample[key] = {"raw": raw, "parsed": json.loads(raw)}
                        save(output / f"{stage}.json", sample)
                        validate_population(sample["server_info"]["parsed"],
                                            sample["members"]["parsed"], sid)
                        sample["smaps_rollup"] = smaps(server.pid)
                        sample["smaps"] = full_smaps(server.pid)
                        save(output / f"{stage}.json", sample)
                        result["server_build"] = sample["server_info"]["parsed"]
                        for key, command in (("server_memory", "server:memory"),
                                             ("allocator", "allocator"),
                                             ("agent_memory", "agent:memory")):
                            if key == "allocator" and sid is None:
                                sample[key] = {
                                    "source": "server:memory.process.allocator (allocator command requires a session)",
                                    "parsed": sample["server_memory"]["parsed"]["process"]["allocator"]}
                                continue
                            if key == "agent_memory" and sid is None:
                                sample[key] = {"unavailable": "startup has no session"}
                                continue
                            raw = cmd(command, sid)
                            sample[key] = {"raw": raw, "parsed": json.loads(raw)}
                            save(output / f"{stage}.json", sample)
                        if args.heap_profile and sid is not None:
                            heap_path = output / f"{stage}.heap"
                            raw = cmd(f"allocator:profile:dump {heap_path}", sid)
                            sample["heap_profile_dump"] = {"raw": raw, "path": str(heap_path)}
                            if not heap_path.is_file():
                                raise RuntimeError(f"heap profile dump missing: {heap_path}")
                        sample["pricing_cache"] = capture_pricing_cache(root / "home", output)
                        save(output / f"{stage}.json", sample)
                        save(output / "profile.json", result)
                        print(f"{stage}: server PSS {sample['smaps_rollup']['values_kib']['Pss']} KiB",
                              flush=True)

                    time.sleep(args.startup_settle_seconds)
                    if args.require_pricing_refresh:
                        deadline = time.monotonic() + args.timeout
                        cache_path = root / "home" / "cache" / "models_dev_pricing.json"
                        while not cache_path.is_file():
                            if server.poll() is not None:
                                raise RuntimeError("server exited while waiting for pricing refresh")
                            if time.monotonic() >= deadline:
                                raise TimeoutError("fresh pricing catalog not published")
                            time.sleep(0.05)
                    started = time.monotonic()
                    capture("startup")
                    created = cmd(f"create_session:{args.cwd}")
                    save(output / "create_session.json", {"raw": created})
                    sid = json.loads(created)["session_id"]
                    result["session_id"] = sid
                    if args.model:
                        save(output / "set_model.json", {"raw": cmd(f"set_model:{args.model}", sid)})
                    capture("session_idle", sid)
                    for turn in range(1, args.turns + 1):
                        raw = cmd(f"message:{args.prompt}", sid)
                        save(output / f"turn_{turn:03d}_response.json", {"raw": raw})
                        capture(f"turn_{turn:03d}", sid)
                    time.sleep(args.settle_seconds)
                    capture("settled_idle", sid)
                    capture("before_explicit_purge", sid)
                    save(output / "explicit_purge.json", {"raw": cmd("allocator:purge", sid)})
                    capture("after_explicit_purge", sid)
                    result["status"] = "complete"
                finally:
                    try:
                        os.killpg(server.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        server.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        os.killpg(server.pid, signal.SIGKILL)
                        server.wait()
    except BaseException as exc:
        result["status"] = "failed"
        result["error"] = f"{type(exc).__name__}: {exc}"
        raise
    finally:
        save(output / "profile.json", result)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--cwd", type=Path, required=True)
    parser.add_argument("--turns", type=int, required=True, help="real model turns, zero for idle-only")
    parser.add_argument("--output", type=Path, required=True, help="new artifact directory, must not exist")
    parser.add_argument("--auth-from", type=Path, default=Path.home() / ".jcode",
                        help="copy known auth files from this directory, never config")
    parser.add_argument("--model", help="explicit provider/model selector")
    parser.add_argument("--prompt", default="Reply with exactly the word: done. Do not use tools.")
    parser.add_argument("--startup-settle-seconds", type=float, default=3,
                        help="wait after debug socket readiness before startup baseline (default: 3)")
    parser.add_argument("--heap-profile", action="store_true",
                        help="enable startup jemalloc sampling and post-session dumps (requires jemalloc-prof build)")
    parser.add_argument("--require-pricing-refresh", action="store_true",
                        help="wait for the cold pricing refresh before measuring, fail if it never succeeds")
    parser.add_argument("--settle-seconds", type=float, default=10)
    parser.add_argument("--timeout", type=float, default=600)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.cwd = args.cwd.resolve()
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("--binary must be an executable file")
    if not args.cwd.is_dir() or args.turns < 0:
        parser.error("--cwd must be a directory and --turns must be nonnegative")
    if (not math.isfinite(args.startup_settle_seconds) or args.startup_settle_seconds < 0 or
            not math.isfinite(args.settle_seconds) or args.settle_seconds < 0 or
            not math.isfinite(args.timeout) or args.timeout <= 0):
        parser.error("settle seconds must be finite/nonnegative and timeout finite/positive")
    try:
        run(args)
    except (Exception, KeyboardInterrupt) as exc:
        print(f"profile failed: {exc}", file=sys.stderr)
        return 1
    print(f"Artifacts: {args.output.resolve()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
