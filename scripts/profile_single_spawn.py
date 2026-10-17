#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import socket
import subprocess
import tempfile
import time
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Profile a single resumed jcode spawn")
    parser.add_argument("--binary", default="./target/selfdev/jcode")
    parser.add_argument("--timeout", type=float, default=20.0)
    parser.add_argument("--cwd", default=os.getcwd())
    parser.add_argument("--json", action="store_true", help="Emit JSON summary")
    return parser.parse_args()


def wait_for_socket(path: Path, timeout_s: float = 10.0) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if path.exists():
            try:
                sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                sock.settimeout(0.2)
                sock.connect(str(path))
                sock.close()
                return
            except OSError:
                pass
        time.sleep(0.01)
    raise RuntimeError(f"socket not ready: {path}")


def create_session(debug_sock: Path, cwd: str) -> str:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(str(debug_sock))
    req = {"type": "debug_command", "id": 1, "command": f"create_session:{cwd}"}
    sock.sendall((json.dumps(req) + "\n").encode())
    buf = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            resp = json.loads(line.decode())
            if resp.get("type") == "ack":
                continue
            if resp.get("type") == "error":
                raise RuntimeError(resp.get("message") or resp)
            if resp.get("type") != "debug_response":
                continue
            if not resp.get("ok", True):
                raise RuntimeError(resp.get("output") or resp)
            output = json.loads(resp["output"])
            return output["session_id"]
    raise RuntimeError("missing debug response")


def reply_queries(master_fd: int, buffer: bytes) -> bytes:
    replies = [
        (b"\x1b[6n", b"\x1b[1;1R"),
        (b"\x1b[c", b"\x1b[?62;c"),
        (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
        (b"\x1b]11;?\x1b\\", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
        (b"\x1b]10;?\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
        (b"\x1b]11;?\x07", b"\x1b]11;rgb:0000/0000/0000\x07"),
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


def latest_log_file(log_dir: Path) -> Path:
    logs = sorted(log_dir.glob("jcode-*.log"), key=lambda p: p.stat().st_mtime)
    if not logs:
        raise RuntimeError(f"no log files found in {log_dir}")
    return logs[-1]


def extract_timing_lines(log_path: Path) -> list[str]:
    timing_lines = []
    for line in log_path.read_text(errors="replace").splitlines():
        if "[TIMING]" in line or "Startup Profile (" in line:
            timing_lines.append(line)
    return timing_lines


def profile_single_spawn(binary: str, cwd: str, timeout_s: float) -> dict:
    root = Path(tempfile.mkdtemp(prefix="jcode-single-profile-"))
    home = root / "home"
    runtime_dir = root / "run"
    socket_path = runtime_dir / "kcode.sock"
    debug_socket_path = runtime_dir / "kcode-debug.sock"
    home.mkdir(parents=True, exist_ok=True)
    runtime_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(
        {
            "KCODE_HOME": str(home),
            "KCODE_RUNTIME_DIR": str(runtime_dir),
            "KCODE_SOCKET": str(socket_path),
            "KCODE_DEBUG_SOCKET": str(debug_socket_path),
            "KCODE_SWARM_ENABLED": "0",
            "KCODE_NO_TELEMETRY": "1",
            "KCODE_TRACE": "1",
            "KCODE_TEMP_SERVER": "1",
            "KCODE_SERVER_OWNER_PID": str(os.getpid()),
        }
    )

    server_proc = subprocess.Popen(
        [binary, "--no-update", "--no-selfdev", "serve", "--socket", str(socket_path)],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        preexec_fn=os.setsid,
    )
    try:
        wait_for_socket(socket_path, timeout_s=min(timeout_s, 10.0))
        wait_for_socket(debug_socket_path, timeout_s=min(timeout_s, 10.0))
        session_id = create_session(debug_socket_path, cwd)

        master_fd, slave_fd = pty.openpty()
        client_start = time.perf_counter()
        client_proc = subprocess.Popen(
            [
                binary,
                "--no-update",
                "--no-selfdev",
                "--socket",
                str(socket_path),
                "--fresh-spawn",
                "--resume",
                session_id,
            ],
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            env=env,
            preexec_fn=os.setsid,
        )
        os.close(slave_fd)
        os.set_blocking(master_fd, False)

        buffer = b""
        first_output_ms = None
        last_output_at = None
        deadline = time.perf_counter() + timeout_s
        settle_after_output_s = 0.2
        while time.perf_counter() < deadline:
            if client_proc.poll() is not None:
                break
            rlist, _, _ = select.select([master_fd], [], [], 0.05)
            if rlist:
                chunk = os.read(master_fd, 65536)
                if not chunk:
                    break
                if first_output_ms is None:
                    first_output_ms = (time.perf_counter() - client_start) * 1000.0
                last_output_at = time.perf_counter()
                buffer += chunk
                buffer = reply_queries(master_fd, buffer)
                lower = buffer.lower()
                if b"loading session" in lower or b"jcode" in lower or len(buffer) > 4096:
                    if time.perf_counter() - last_output_at >= settle_after_output_s:
                        break
            elif last_output_at is not None and time.perf_counter() - last_output_at >= settle_after_output_s:
                break

        os.close(master_fd)
        try:
            os.killpg(client_proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            client_proc.wait(timeout=0.2)
        except Exception:
            try:
                os.killpg(client_proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        log_path = latest_log_file(home / "logs")
        return {
            "temp_root": str(root),
            "log_path": str(log_path),
            "session_id": session_id,
            "first_output_ms": first_output_ms,
            "timing_lines": extract_timing_lines(log_path),
        }
    finally:
        try:
            os.killpg(server_proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            server_proc.wait(timeout=1.0)
        except Exception:
            try:
                os.killpg(server_proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def main() -> None:
    args = parse_args()
    result = profile_single_spawn(args.binary, args.cwd, args.timeout)
    if args.json:
        print(json.dumps(result, indent=2))
        return

    print(f"temp root: {result['temp_root']}")
    print(f"session id: {result['session_id']}")
    print(f"log path: {result['log_path']}")
    if result["first_output_ms"] is not None:
        print(f"first output: {result['first_output_ms']:.1f}ms")
    else:
        print("first output: none")
    print("timings:")
    for line in result["timing_lines"]:
        print(f"  {line}")


if __name__ == "__main__":
    main()
