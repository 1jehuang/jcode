#!/usr/bin/env python3
"""Benchmark interactive CLI startup using user-visible metrics.

Measures two UX-focused metrics for interactive PTY launches:
1. time to first visible content
2. time until typed probe text appears on the rendered screen (input-ready)

The benchmark drives a pseudo-terminal, answers common terminal capability
queries, renders the output through a terminal screen model, and detects when
meaningful text becomes visible.
"""

from __future__ import annotations

import argparse
import json
import os
import pty
import select
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time
from dataclasses import dataclass
from pathlib import Path

try:
    import fcntl
except ImportError as exc:  # pragma: no cover
    raise SystemExit(f"fcntl unavailable: {exc}")

try:
    import pyte
except ImportError as exc:  # pragma: no cover
    raise SystemExit(
        "pyte is required. Install with: python3 -m pip install pyte\n"
        f"Import error: {exc}"
    )

PROBE = "jqx92"
DEFAULT_RUNS = 10
DEFAULT_TIMEOUT_S = 10.0


@dataclass(frozen=True)
class ToolSpec:
    name: str
    argv: list[str]
    no_telem_env: dict[str, str] | None = None
    disable_selfdev: bool = False
    input_ready_log_marker: str | None = None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=DEFAULT_RUNS)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_S)
    parser.add_argument("--cwd", default=os.getcwd())
    parser.add_argument(
        "--tools",
        nargs="*",
        default=None,
        help="subset of tool names to benchmark",
    )
    parser.add_argument(
        "--json-out",
        default="/var/tmp/startup_visible_ready_results.json",
        help="where to write the JSON results",
    )
    return parser.parse_args()


def detect_pi_bin() -> str:
    pi = shutil_which("pi")
    if pi:
        return pi
    prefix = subprocess.check_output(["npm", "prefix", "-g"], text=True).strip()
    candidate = Path(prefix) / "bin" / "pi"
    if candidate.exists():
        return str(candidate)
    raise FileNotFoundError("could not find pi binary")


def shutil_which(name: str) -> str | None:
    return subprocess.run(
        ["bash", "-lc", f"command -v {name}"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip() or None


def build_tool_specs() -> list[ToolSpec]:
    specs = [
        ToolSpec(
            name="jcode",
            argv=["jcode", "--no-update", "--no-selfdev"],
            no_telem_env={"JCODE_NO_TELEMETRY": "1"},
            disable_selfdev=True,
        ),
        ToolSpec(name="pi", argv=[detect_pi_bin()]),
        ToolSpec(name="opencode", argv=["opencode"]),
        ToolSpec(name="codex", argv=["codex"]),
        ToolSpec(name="claude_code", argv=["claude"]),
        ToolSpec(name="cursor_agent", argv=["cursor-agent"]),
        ToolSpec(name="copilot_cli", argv=["copilot"]),
        ToolSpec(
            name="antigravity_cli",
            argv=["agy"],
            no_telem_env={"AGY_CLI_DISABLE_AUTO_UPDATE": "1"},
            input_ready_log_marker="CLI ready for user input",
        ),
    ]
    return specs


def configure_pty(slave_fd: int, rows: int = 24, cols: int = 80) -> None:
    fcntl.ioctl(slave_fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    attrs = termios.tcgetattr(slave_fd)
    attrs[3] &= ~(termios.ECHO | termios.ICANON)
    attrs[0] &= ~(termios.ICRNL | termios.IXON)
    termios.tcsetattr(slave_fd, termios.TCSANOW, attrs)


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


def first_meaningful_line(screen: pyte.Screen) -> str | None:
    for line in screen.display:
        normalized = " ".join(line.split())
        if not normalized or PROBE in normalized:
            continue
        alnum_count = sum(ch.isalnum() for ch in normalized)
        if alnum_count >= 3 and len(normalized) >= 4:
            return normalized[:120]
    return None


def run_once(spec: ToolSpec, cwd: Path, timeout_s: float) -> dict[str, object]:
    master_fd, slave_fd = pty.openpty()
    configure_pty(slave_fd)
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    env["COLORTERM"] = "truecolor"
    if spec.no_telem_env:
        env.update(spec.no_telem_env)
    argv = spec.argv
    input_ready_log_path: Path | None = None
    if spec.input_ready_log_marker:
        log_file = tempfile.NamedTemporaryFile(prefix=f"{spec.name}-", suffix=".log", delete=False)
        input_ready_log_path = Path(log_file.name)
        log_file.close()
        argv = [*spec.argv, "--log-file", str(input_ready_log_path)]
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

    screen = pyte.Screen(80, 24)
    stream = pyte.Stream(screen)
    start = time.perf_counter()
    query_buffer = b""
    first_visible_ms: float | None = None
    first_visible_excerpt: str | None = None
    input_ready_ms: float | None = None
    probe_sent = False

    try:
        while time.perf_counter() - start < timeout_s:
            rlist, _, _ = select.select([master_fd], [], [], 0.05)
            if rlist:
                try:
                    chunk = os.read(master_fd, 65536)
                except BlockingIOError:
                    chunk = b""
                if chunk:
                    query_buffer += chunk
                    query_buffer = reply_queries(master_fd, query_buffer)
                    stream.feed(chunk.decode("utf-8", "replace"))
            if first_visible_ms is None:
                excerpt = first_meaningful_line(screen)
                if excerpt:
                    first_visible_ms = (time.perf_counter() - start) * 1000.0
                    first_visible_excerpt = excerpt
                    os.write(master_fd, PROBE.encode())
                    probe_sent = True
            elif probe_sent and input_ready_ms is None:
                if PROBE in "\n".join(screen.display):
                    input_ready_ms = (time.perf_counter() - start) * 1000.0
                    break
            if (
                spec.input_ready_log_marker
                and first_visible_ms is not None
                and input_ready_ms is None
                and input_ready_log_path
            ):
                try:
                    if spec.input_ready_log_marker in input_ready_log_path.read_text(errors="replace"):
                        input_ready_ms = (time.perf_counter() - start) * 1000.0
                        break
                except OSError:
                    pass
        return {
            "first_visible_ms": first_visible_ms,
            "first_visible_excerpt": first_visible_excerpt,
            "input_ready_ms": input_ready_ms,
            "input_ready_source": "log_marker" if spec.input_ready_log_marker else "probe_echo",
        }
    finally:
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(proc.pid, sig)
                time.sleep(0.1)
            except ProcessLookupError:
                break
        try:
            proc.wait(timeout=1)
        except Exception:
            pass
        os.close(master_fd)
        if input_ready_log_path:
            try:
                input_ready_log_path.unlink()
            except OSError:
                pass


def summarize(samples: list[float | None]) -> dict[str, float | int] | None:
    values = [sample for sample in samples if sample is not None]
    if not values:
        return None
    return {
        "median_ms": statistics.median(values),
        "min_ms": min(values),
        "max_ms": max(values),
        "mean_ms": statistics.mean(values),
        "runs_completed": len(values),
        "runs_total": len(samples),
    }


def version_for(spec: ToolSpec) -> str:
    argv = spec.argv[:1]
    if spec.name == "jcode":
        argv = [spec.argv[0], "version"]
    else:
        argv = [spec.argv[0], "--version"]
    proc = subprocess.run(argv, capture_output=True, text=True, check=False)
    output = (proc.stdout + proc.stderr).strip().splitlines()
    return output[0] if output else f"exit {proc.returncode}"


def main() -> None:
    args = parse_args()
    selected = set(args.tools or [])
    specs = build_tool_specs()
    if selected:
        specs = [spec for spec in specs if spec.name in selected]
    cwd = Path(args.cwd).resolve()

    results: dict[str, object] = {
        "runs": args.runs,
        "timeout_s": args.timeout,
        "cwd": str(cwd),
        "tools": {},
    }
    for spec in specs:
        print(f"=== {spec.name} ===", flush=True)
        runs: list[dict[str, object]] = []
        for i in range(args.runs):
            run = run_once(spec, cwd, args.timeout)
            runs.append(run)
            print(
                f"run {i + 1}/{args.runs}: "
                f"visible={run['first_visible_ms']} ready={run['input_ready_ms']} "
                f"excerpt={run['first_visible_excerpt']}",
                flush=True,
            )
        results["tools"][spec.name] = {
            "version": version_for(spec),
            "runs": runs,
            "first_visible_summary": summarize([r["first_visible_ms"] for r in runs]),
            "input_ready_summary": summarize([r["input_ready_ms"] for r in runs]),
        }

    out_path = Path(args.json_out)
    out_path.write_text(json.dumps(results, indent=2))
    print(f"WROTE {out_path}", flush=True)


if __name__ == "__main__":
    main()
