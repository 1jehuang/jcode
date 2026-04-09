#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import socket
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

CLK_TCK = os.sysconf("SC_CLK_TCK")
PAGE_SIZE = os.sysconf("SC_PAGE_SIZE")


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Profile resumed jcode PTY burst startup")
    p.add_argument("--binary", default="./target/release/jcode")
    p.add_argument("--burst", type=int, default=20)
    p.add_argument("--timeout", type=float, default=15.0)
    p.add_argument("--json-out", default="/tmp/jcode_remote_burst_profile.json")
    return p.parse_args()


@dataclass
class ProcSample:
    cpu_ticks: int
    rss_kb: int


@dataclass
class ProcTracker:
    start_cpu_ticks: int | None = None
    last_cpu_ticks: int = 0
    peak_rss_kb: int = 0

    def record(self, sample: ProcSample | None) -> bool:
        if sample is None:
            return False
        if self.start_cpu_ticks is None:
            self.start_cpu_ticks = sample.cpu_ticks
        self.last_cpu_ticks = sample.cpu_ticks
        self.peak_rss_kb = max(self.peak_rss_kb, sample.rss_kb)
        return True

    def cpu_ms(self) -> float:
        if self.start_cpu_ticks is None:
            return 0.0
        return (self.last_cpu_ticks - self.start_cpu_ticks) * 1000.0 / CLK_TCK


def read_proc_sample(pid: int) -> ProcSample | None:
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except FileNotFoundError:
        return None
    end = stat.rfind(")")
    if end == -1:
        return None
    fields = stat[end + 2 :].split()
    if len(fields) <= 21:
        return None
    utime = int(fields[11])
    stime = int(fields[12])
    rss_pages = int(fields[21])
    return ProcSample(cpu_ticks=utime + stime, rss_kb=rss_pages * PAGE_SIZE // 1024)


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


def create_session(debug_sock: Path, cwd: str = ".") -> str:
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


@dataclass
class LiveClient:
    session_id: str
    proc: subprocess.Popen
    master_fd: int
    start: float
    buffer: bytes
    tracker: ProcTracker
    first_output_ms: float | None = None
    done: bool = False


def start_resume_client(binary: str, env: dict[str, str], session_id: str) -> LiveClient:
    master_fd, slave_fd = pty.openpty()
    start = time.perf_counter()
    proc = subprocess.Popen(
        [
            binary,
            "--no-update",
            "--no-selfdev",
            "--socket",
            env["JCODE_SOCKET"],
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
    return LiveClient(
        session_id=session_id,
        proc=proc,
        master_fd=master_fd,
        start=start,
        buffer=b"",
        tracker=ProcTracker(),
    )


def finish_client(client: LiveClient) -> dict:
    try:
        client.tracker.record(read_proc_sample(client.proc.pid))
        return {
            "session_id": client.session_id,
            "pid": client.proc.pid,
            "first_output_ms": client.first_output_ms,
            "buffer_bytes": len(client.buffer),
            "cpu_ms": client.tracker.cpu_ms(),
            "peak_rss_kb": client.tracker.peak_rss_kb,
        }
    finally:
        os.close(client.master_fd)
        try:
            os.killpg(client.proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            client.proc.wait(timeout=1)
        except Exception:
            try:
                os.killpg(client.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def run_burst(
    binary: str, env: dict[str, str], session_ids: list[str], timeout_s: float, server_pid: int
) -> tuple[list[dict], dict[str, float | int]]:
    clients = [start_resume_client(binary, env, session_id) for session_id in session_ids]
    fd_to_index = {client.master_fd: idx for idx, client in enumerate(clients)}
    deadline = time.perf_counter() + timeout_s
    server_tracker = ProcTracker()
    peak_clients_rss_kb = 0
    peak_live_clients = 0

    def sample_processes() -> None:
        nonlocal peak_clients_rss_kb, peak_live_clients
        server_tracker.record(read_proc_sample(server_pid))
        live_clients = 0
        clients_rss_kb = 0
        for client in clients:
            sample = read_proc_sample(client.proc.pid)
            if client.tracker.record(sample):
                live_clients += 1
                clients_rss_kb += sample.rss_kb if sample is not None else 0
        peak_live_clients = max(peak_live_clients, live_clients)
        peak_clients_rss_kb = max(peak_clients_rss_kb, clients_rss_kb)

    sample_processes()

    while time.perf_counter() < deadline and any(not client.done for client in clients):
        active_fds = [client.master_fd for client in clients if not client.done]
        if not active_fds:
            break
        rlist, _, _ = select.select(active_fds, [], [], 0.05)
        sample_processes()
        for fd in rlist:
            client = clients[fd_to_index[fd]]
            try:
                chunk = os.read(fd, 65536)
            except BlockingIOError:
                chunk = b""
            if not chunk:
                client.done = True
                continue
            if client.first_output_ms is None:
                client.first_output_ms = (time.perf_counter() - client.start) * 1000.0
            client.buffer += chunk
            client.buffer = reply_queries(fd, client.buffer)
            lower = client.buffer.lower()
            if b"loading session" in lower or b"jcode" in lower or len(client.buffer) > 2048:
                client.done = True

        for client in clients:
            if client.done:
                continue
            if client.proc.poll() is not None:
                client.done = True

    for client in clients:
        client.done = True

    sample_processes()

    results = [finish_client(client) for client in clients]
    metrics = {
        "server_cpu_ms": server_tracker.cpu_ms(),
        "server_peak_rss_kb": server_tracker.peak_rss_kb,
        "clients_cpu_ms": sum(result["cpu_ms"] for result in results),
        "clients_peak_rss_kb": peak_clients_rss_kb,
        "peak_live_clients": peak_live_clients,
    }
    return results, metrics


def main() -> None:
    args = parse_args()
    root = Path(tempfile.mkdtemp(prefix="jcode-remote-burst-"))
    home = root / "home"
    run = root / "run"
    home.mkdir(parents=True, exist_ok=True)
    run.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["JCODE_HOME"] = str(home)
    env["JCODE_RUNTIME_DIR"] = str(run)
    env["JCODE_SOCKET"] = str(run / "jcode.sock")
    env["JCODE_NO_TELEMETRY"] = "1"
    env["JCODE_DEBUG_CONTROL"] = "1"
    debug_socket = run / "jcode-debug.sock"

    server = subprocess.Popen(
        [args.binary, "serve", "--socket", env["JCODE_SOCKET"], "--debug-socket"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        preexec_fn=os.setsid,
    )
    try:
        wait_for_socket(Path(env["JCODE_SOCKET"]))
        wait_for_socket(debug_socket)
        session_ids = [create_session(debug_socket, os.getcwd()) for _ in range(args.burst)]

        wall_start = time.perf_counter()
        results, proc_metrics = run_burst(args.binary, env, session_ids, args.timeout, server.pid)
        wall_ms = (time.perf_counter() - wall_start) * 1000.0
        firsts = [r["first_output_ms"] for r in results if r["first_output_ms"] is not None]
        output = {
            "burst": args.burst,
            "wall_ms": wall_ms,
            "server_cpu_ms": proc_metrics["server_cpu_ms"],
            "clients_cpu_ms": proc_metrics["clients_cpu_ms"],
            "total_cpu_ms": proc_metrics["server_cpu_ms"] + proc_metrics["clients_cpu_ms"],
            "cpu_utilization_ratio": 0.0
            if wall_ms == 0
            else (proc_metrics["server_cpu_ms"] + proc_metrics["clients_cpu_ms"]) / wall_ms,
            "first_output_ms": {
                "min": min(firsts) if firsts else None,
                "p50": statistics.median(firsts) if firsts else None,
                "max": max(firsts) if firsts else None,
            },
            "buffer_bytes_total": sum(r["buffer_bytes"] for r in results),
            "server_peak_rss_kb": proc_metrics["server_peak_rss_kb"],
            "clients_peak_rss_kb": proc_metrics["clients_peak_rss_kb"],
            "peak_live_clients": proc_metrics["peak_live_clients"],
            "results": results,
        }
        Path(args.json_out).write_text(json.dumps(output, indent=2))
        print(json.dumps(output, indent=2))
    finally:
        try:
            os.killpg(server.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            server.wait(timeout=2)
        except Exception:
            try:
                os.killpg(server.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


if __name__ == "__main__":
    main()
