#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import pty
import re
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path

ANSI_RE = re.compile(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\x1b\x07]*(?:\x07|\x1b\\))")
PROBE = "jqx92"
DEFAULT_TIMEOUT_S = 20.0
DEFAULT_SETTLE_S = 1.0
DEFAULT_TOOLS = [
    "jcode_memory_off",
    "jcode_memory_on",
    "pi",
    "codex",
    "opencode",
    "copilot_cli",
    "cursor_agent",
    "claude_code",
    "antigravity_cli",
]


@dataclass
class ToolSpec:
    name: str
    argv: list[str]
    version_argv: list[str]
    env: dict[str, str] | None = None
    jcode: bool = False


@dataclass
class SessionLaunch:
    root_pid: int
    pgid: int
    master_fd: int
    ready: bool
    input_ready: bool
    excerpt: str | None
    seconds_to_visible: float | None
    seconds_to_input_ready: float | None
    buffer_excerpt: str | None


@dataclass
class ToolRunResult:
    tool: str
    sessions: int
    pss_mb: float
    process_count: int
    version: str
    notes: list[str]


def shutil_which(name: str) -> str | None:
    return subprocess.run(
        ["bash", "-lc", f"command -v {name}"], capture_output=True, text=True, check=False
    ).stdout.strip() or None


def detect_pi_bin() -> str:
    direct = shutil_which("pi")
    if direct:
        return direct
    prefix = subprocess.check_output(["npm", "prefix", "-g"], text=True).strip()
    candidate = Path(prefix) / "bin" / "pi"
    if candidate.exists():
        return str(candidate)
    raise FileNotFoundError("could not find pi binary")


def build_specs() -> dict[str, ToolSpec]:
    jcode = shutil.which("jcode") or str(Path.home() / ".local/bin/jcode")
    codex = shutil.which("codex") or "/usr/bin/codex"
    opencode = shutil.which("opencode") or "/usr/bin/opencode"
    copilot = shutil.which("copilot") or str(Path.home() / ".local/bin/copilot")
    cursor_agent = shutil.which("cursor-agent") or str(Path.home() / ".local/bin/cursor-agent")
    claude = shutil.which("claude") or str(Path.home() / ".local/bin/claude")
    agy = shutil.which("agy") or str(Path.home() / ".local/bin/agy")
    specs = {
        "jcode_memory_off": ToolSpec(
            name="jcode_memory_off",
            argv=[jcode, "--no-update", "--no-selfdev"],
            version_argv=[jcode, "version"],
            env={"JCODE_NO_TELEMETRY": "1", "JCODE_MEMORY_ENABLED": "0"},
            jcode=True,
        ),
        "jcode_memory_on": ToolSpec(
            name="jcode_memory_on",
            argv=[jcode, "--no-update", "--no-selfdev"],
            version_argv=[jcode, "version"],
            env={"JCODE_NO_TELEMETRY": "1", "JCODE_MEMORY_ENABLED": "1"},
            jcode=True,
        ),
        "pi": ToolSpec(
            name="pi",
            argv=[detect_pi_bin()],
            version_argv=[detect_pi_bin(), "--version"],
        ),
        "codex": ToolSpec(
            name="codex",
            argv=[codex],
            version_argv=[codex, "--version"],
        ),
        "opencode": ToolSpec(
            name="opencode",
            argv=[opencode],
            version_argv=[opencode, "--version"],
        ),
        "copilot_cli": ToolSpec(
            name="copilot_cli",
            argv=[copilot],
            version_argv=[copilot, "--version"],
        ),
        "cursor_agent": ToolSpec(
            name="cursor_agent",
            argv=[cursor_agent],
            version_argv=[cursor_agent, "--version"],
        ),
        "claude_code": ToolSpec(
            name="claude_code",
            argv=[claude],
            version_argv=[claude, "--version"],
        ),
        "antigravity_cli": ToolSpec(
            name="antigravity_cli",
            argv=[agy],
            version_argv=[agy, "--version"],
        ),
    }
    return specs


def reply_queries(master_fd: int, buffer: bytes) -> bytes:
    replies = [
        (b"\x1b[6n", b"\x1b[1;1R"),
        (b"\x1b[c", b"\x1b[?62;c"),
        (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
        (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
        (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
        (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
        (b"\x1b]4;0;?\x07", b"\x1b]4;0;rgb:0000/0000/0000\x07"),
        (b"\x1b[14t", b"\x1b[4;600;800t"),
        (b"\x1b[16t", b"\x1b[6;16;8t"),
        (b"\x1b[18t", b"\x1b[8;24;80t"),
        (b"\x1b[?1016$p", b"\x1b[?1016;1$y"),
        (b"\x1b[?2027$p", b"\x1b[?2027;1$y"),
        (b"\x1b[?2031$p", b"\x1b[?2031;1$y"),
        (b"\x1b[?1004$p", b"\x1b[?1004;1$y"),
        (b"\x1b[?2004$p", b"\x1b[?2004;1$y"),
        (b"\x1b[?2026$p", b"\x1b[?2026;1$y"),
    ]
    changed = True
    while changed:
        changed = False
        for query, response in replies:
            if query in buffer:
                os.write(master_fd, response)
                buffer = buffer.replace(query, b"")
                changed = True
    return buffer


def strip_ansi(text: str) -> str:
    return ANSI_RE.sub("", text).replace("\r", "\n")


def first_meaningful_line(text: str) -> str | None:
    for raw_line in text.splitlines():
        line = " ".join(raw_line.split())
        if not line:
            continue
        alnum_count = sum(ch.isalnum() for ch in line)
        if alnum_count >= 3 and len(line) >= 4:
            return line[:160]
    return None


def wait_for_socket(path: str, timeout_s: float) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if os.path.exists(path):
            try:
                sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                sock.connect(path)
                sock.close()
                return True
            except OSError:
                pass
        time.sleep(0.05)
    return False


def launch_interactive(argv: list[str], cwd: Path, env: dict[str, str], timeout_s: float, settle_s: float) -> SessionLaunch:
    master_fd, slave_fd = pty.openpty()
    proc = subprocess.Popen(
        argv,
        cwd=str(cwd),
        env=env,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        preexec_fn=os.setsid,
    )
    os.close(slave_fd)
    os.set_blocking(master_fd, False)
    start = time.perf_counter()
    buf = b""
    ready = False
    input_ready = False
    probe_sent = False
    excerpt = None
    while time.perf_counter() - start < timeout_s:
        rlist, _, _ = select.select([master_fd], [], [], 0.05)
        if rlist:
            try:
                chunk = os.read(master_fd, 65536)
            except BlockingIOError:
                chunk = b""
            if chunk:
                buf += chunk
                buf = reply_queries(master_fd, buf)
                plain = strip_ansi(buf.decode("utf-8", "replace"))
                excerpt = first_meaningful_line(plain)
                if excerpt:
                    ready = True
                    if not probe_sent:
                        try:
                            os.write(master_fd, PROBE.encode())
                            probe_sent = True
                        except OSError:
                            break
                if probe_sent and PROBE in plain:
                    input_ready = True
                    break
        if proc.poll() is not None:
            break
    if input_ready or ready:
        time.sleep(settle_s)
    elapsed = time.perf_counter() - start
    return SessionLaunch(
        root_pid=proc.pid,
        pgid=os.getpgid(proc.pid),
        master_fd=master_fd,
        ready=ready,
        input_ready=input_ready,
        excerpt=excerpt,
        seconds_to_visible=elapsed if ready else None,
        seconds_to_input_ready=elapsed if input_ready else None,
        buffer_excerpt=(strip_ansi(buf.decode("utf-8", "replace"))[:300] or None),
    )


def iter_proc_stat() -> dict[int, tuple[int, int]]:
    out: dict[int, tuple[int, int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except Exception:
            continue
        try:
            close = stat.rfind(")")
            rest = stat[close + 2 :].split()
            ppid = int(rest[1])
            pgid = int(rest[2])
            out[int(entry.name)] = (ppid, pgid)
        except Exception:
            continue
    return out


def collect_descendants(root_pids: list[int]) -> set[int]:
    ppid_of = iter_proc_stat()
    children: dict[int, list[int]] = {}
    for pid, (ppid, _pgid) in ppid_of.items():
        children.setdefault(ppid, []).append(pid)
    seen: set[int] = set()
    stack = list(root_pids)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        stack.extend(children.get(pid, []))
    return seen


def collect_process_group_pids(pgids: list[int]) -> set[int]:
    proc_map = iter_proc_stat()
    wanted = set(pgids)
    return {pid for pid, (_ppid, pgid) in proc_map.items() if pgid in wanted}


def read_pss_mb(pid: int) -> float | None:
    path = Path(f"/proc/{pid}/smaps_rollup")
    try:
        for line in path.read_text().splitlines():
            if line.startswith("Pss:"):
                return int(line.split()[1]) / 1024.0
    except Exception:
        return None
    return None


def sum_tree_pss(root_pids: list[int], pgids: list[int]) -> tuple[float, int]:
    all_pids = collect_descendants(root_pids) | collect_process_group_pids(pgids)
    total = 0.0
    counted = 0
    for pid in sorted(all_pids):
        pss = read_pss_mb(pid)
        if pss is None:
            continue
        total += pss
        counted += 1
    return round(total, 1), counted


def terminate_pgroup(pgid: int) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pgid, sig)
            time.sleep(0.2)
        except ProcessLookupError:
            return


def version_for(spec: ToolSpec) -> str:
    proc = subprocess.run(spec.version_argv, capture_output=True, text=True, check=False)
    output = (proc.stdout + proc.stderr).strip().splitlines()
    return output[0] if output else f"exit {proc.returncode}"


def run_tool(spec: ToolSpec, sessions: int, cwd: Path, timeout_s: float, settle_s: float) -> ToolRunResult:
    notes: list[str] = []
    version = version_for(spec)
    launches: list[SessionLaunch] = []
    cleanup_pgids: list[int] = []
    temp_root: str | None = None
    try:
        if spec.jcode:
            temp_root = tempfile.mkdtemp(prefix="jcode-memory-bench-")
            env = os.environ.copy()
            if spec.env:
                env.update(spec.env)
            env["JCODE_HOME"] = os.path.join(temp_root, "home")
            env["JCODE_RUNTIME_DIR"] = os.path.join(temp_root, "run")
            env["JCODE_TEMP_SERVER"] = "1"
            env["JCODE_SERVER_OWNER_PID"] = str(os.getpid())
            os.makedirs(env["JCODE_HOME"], exist_ok=True)
            os.makedirs(env["JCODE_RUNTIME_DIR"], exist_ok=True)
            for auth_name in (
                "anthropic-auth.json",
                "openai-auth.json",
                "antigravity_oauth.json",
                "gemini_oauth.json",
                "config.toml",
            ):
                real_auth = Path.home() / ".jcode" / auth_name
                bench_auth = Path(env["JCODE_HOME"]) / auth_name
                if real_auth.exists() and not bench_auth.exists():
                    bench_auth.symlink_to(real_auth)
            if spec.name == "jcode_memory_on":
                real_models = Path.home() / ".jcode" / "models"
                bench_models = Path(env["JCODE_HOME"]) / "models"
                if real_models.exists() and not bench_models.exists():
                    bench_models.symlink_to(real_models)
            socket_path = os.path.join(env["JCODE_RUNTIME_DIR"], "bench.sock")
            server_proc = subprocess.Popen(
                [spec.argv[0], "--no-update", "--no-selfdev", "serve", "--socket", socket_path],
                cwd=str(cwd),
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                preexec_fn=os.setsid,
            )
            cleanup_pgids.append(os.getpgid(server_proc.pid))
            if not wait_for_socket(socket_path, timeout_s):
                raise RuntimeError("jcode server did not become ready")
            if spec.name == "jcode_memory_on":
                time.sleep(max(settle_s, 5.0))
            per_session_settle = max(settle_s, 2.0) if spec.name == "jcode_memory_on" else settle_s
            for _ in range(sessions):
                launches.append(
                    launch_interactive(
                        [spec.argv[0], "--no-update", "--no-selfdev", "--socket", socket_path],
                        cwd,
                        env,
                        timeout_s,
                        per_session_settle,
                    )
                )
                cleanup_pgids.append(launches[-1].pgid)
            root_pids = [server_proc.pid] + [launch.root_pid for launch in launches]
            sample_pgids = cleanup_pgids.copy()
        else:
            env = os.environ.copy()
            if spec.env:
                env.update(spec.env)
            for _ in range(sessions):
                launches.append(launch_interactive(spec.argv, cwd, env, timeout_s, settle_s))
                cleanup_pgids.append(launches[-1].pgid)
            root_pids = [launch.root_pid for launch in launches]
            sample_pgids = cleanup_pgids.copy()

        for idx, launch in enumerate(launches, start=1):
            if not launch.ready:
                notes.append(f"session {idx}: no meaningful screen content before timeout")
            elif launch.excerpt:
                notes.append(f"session {idx}: {launch.excerpt}")
        pss_mb, process_count = sum_tree_pss(root_pids, sample_pgids)
        return ToolRunResult(
            tool=spec.name,
            sessions=sessions,
            pss_mb=pss_mb,
            process_count=process_count,
            version=version,
            notes=notes,
        )
    finally:
        for launch in launches:
            try:
                os.close(launch.master_fd)
            except Exception:
                pass
        for pgid in reversed(cleanup_pgids):
            terminate_pgroup(pgid)
        if temp_root:
            shutil.rmtree(temp_root, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark interactive CLI memory using process-tree PSS")
    parser.add_argument("--sessions", type=int, required=True)
    parser.add_argument("--tools", nargs="*", default=DEFAULT_TOOLS)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_S)
    parser.add_argument("--settle", type=float, default=DEFAULT_SETTLE_S)
    parser.add_argument("--cwd", default=os.getcwd())
    parser.add_argument("--json-out", default=None)
    args = parser.parse_args()

    specs = build_specs()
    cwd = Path(args.cwd).resolve()
    results = []
    for name in args.tools:
        spec = specs[name]
        print(f"=== {name} ({args.sessions} session{'s' if args.sessions != 1 else ''}) ===", flush=True)
        result = run_tool(spec, args.sessions, cwd, args.timeout, args.settle)
        print(json.dumps(asdict(result), indent=2), flush=True)
        results.append(asdict(result))
    payload = {"cwd": str(cwd), "sessions": args.sessions, "results": results}
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(payload, indent=2))
    else:
        print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
