#!/usr/bin/env python3
"""Isolated, no-model-call TUI resource comparison for Jcode and Maki.

Requires pyte. Uses fresh HOME/XDG directories and a dummy Anthropic key.
Never submits a prompt. Jcode's dedicated server is included in every total.
"""
from __future__ import annotations

import argparse
import codecs
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import random
import select
import shutil
import signal
import socket
import statistics
import struct
import subprocess
import tempfile
import termios
import time

import pyte

from bench_memory_cli import collect_descendants, reply_queries


def snapshot(roots):
    totals = dict(pss_mib=0.0, rss_mib=0.0, private_mib=0.0, cpu_seconds=0.0, processes=0)
    for pid in collect_descendants(roots):
        try:
            base = Path(f"/proc/{pid}")
            stat = (base / "stat").read_text().rsplit(")", 1)[1].split()
            memory = dict(line.split(":", 1) for line in (base / "smaps_rollup").read_text().splitlines() if ":" in line)
            totals["pss_mib"] += int(memory["Pss"].split()[0]) / 1024
            totals["rss_mib"] += int(memory["Rss"].split()[0]) / 1024
            totals["private_mib"] += sum(int(memory.get(k, "0").split()[0]) for k in ("Private_Clean", "Private_Dirty")) / 1024
            totals["cpu_seconds"] += (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK")
            totals["processes"] += 1
        except (OSError, ValueError, KeyError):
            continue
    return totals


class Terminal:
    def __init__(self, argv, env, cwd):
        master, slave = pty.openpty()
        self.fd = master
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        attrs = termios.tcgetattr(slave)
        attrs[3] &= ~(termios.ECHO | termios.ICANON)
        termios.tcsetattr(slave, termios.TCSANOW, attrs)
        self.start = time.perf_counter()
        self.proc = subprocess.Popen(argv, env=env, cwd=cwd, stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        os.set_blocking(master, False)
        self.screen = pyte.Screen(120, 40)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.pending = b""
        self.visible_ms = None
        self.bytes = 0

    def text(self):
        return "\n".join(self.screen.display)

    def drain(self):
        try:
            chunk = os.read(self.fd, 262144)
        except (BlockingIOError, OSError):
            return
        if not chunk:
            return
        self.bytes += len(chunk)
        self.pending = reply_queries(self.fd, self.pending + chunk)[-256:]
        self.stream.feed(self.decoder.decode(chunk))
        if self.visible_ms is None and sum(c.isalnum() for c in self.text()) > 8:
            self.visible_ms = (time.perf_counter() - self.start) * 1000


def pump(terms, seconds):
    deadline = time.perf_counter() + seconds
    while time.perf_counter() < deadline:
        ready, _, _ = select.select([t.fd for t in terms], [], [], min(0.01, max(0, deadline - time.perf_counter())))
        for t in terms:
            if t.fd in ready:
                t.drain()


def echo(terms, terminal, text, timeout=10):
    start = time.perf_counter()
    os.write(terminal.fd, text.encode())
    while time.perf_counter() - start < timeout:
        pump(terms, 0.002)
        if text in terminal.text():
            return (time.perf_counter() - start) * 1000
        if terminal.proc.poll() is not None:
            break
    raise RuntimeError(f"Input echo failed: {terminal.text()}")


def stop(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
    except ProcessLookupError:
        proc.wait()


def run(tool, binary, sessions, idle_seconds, root, maki_no_splash=False):
    work = Path(tempfile.mkdtemp(prefix=f"{tool}-", dir=root))
    terms, procs = [], []
    env = {k: v for k, v in os.environ.items() if not any(x in k.upper() for x in ("TOKEN", "API_KEY", "AUTH", "JCODE", "MAKI"))}
    env.update(HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "config"), XDG_DATA_HOME=str(work / "data"), XDG_STATE_HOME=str(work / "state"), XDG_CACHE_HOME=str(work / "cache"), TERM="xterm-256color", COLORTERM="truecolor", ANTHROPIC_API_KEY="sk-ant-benchmark-placeholder-not-valid", JCODE_HOME=str(work / "jcode"), JCODE_RUNTIME_DIR=str(work / "runtime"), JCODE_NO_TELEMETRY="1", JCODE_MEMORY_ENABLED="0", JCODE_TEMP_SERVER="1", JCODE_SERVER_OWNER_PID=str(os.getpid()))
    for key in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "JCODE_HOME", "JCODE_RUNTIME_DIR"):
        Path(env[key]).mkdir(parents=True)
    # Model an already-onboarded install, not the first-run action picker.
    (Path(env["JCODE_HOME"]) / "setup_hints.json").write_text(json.dumps({"launch_count": 10, "onboarding_start_choice_shown": True}))
    cwd = work / "workspace"
    cwd.mkdir()
    start = time.perf_counter()
    server_ms = 0.0
    try:
        if tool == "jcode":
            sock = str(work / "runtime" / "bench.sock")
            log = (work / "server.log").open("w")
            with log:
                server = subprocess.Popen([binary, "--no-update", "--no-selfdev", "--provider", "anthropic-api", "serve", "--socket", sock], env=env, cwd=cwd, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
            procs.append(server)
            deadline = time.perf_counter() + 15
            while True:
                try:
                    with socket.socket(socket.AF_UNIX) as conn:
                        conn.connect(sock)
                    break
                except OSError:
                    if time.perf_counter() > deadline or server.poll() is not None:
                        raise RuntimeError((work / "server.log").read_text())
                    time.sleep(0.002)
            server_ms = (time.perf_counter() - start) * 1000
            argv = [binary, "--no-update", "--no-selfdev", "--socket", sock]
        else:
            argv = [binary, "--no-plugins", "--no-commands", "--model", "anthropic/claude-sonnet-4-6"]
            if maki_no_splash:
                config_dir = Path(env["XDG_CONFIG_HOME"]) / "maki"
                config_dir.mkdir(parents=True, exist_ok=True)
                (config_dir / "init.lua").write_text("maki.setup({ ui = { splash_animation = false } })\n")
                argv.remove("--no-plugins")
        startup = []
        for i in range(sessions):
            t = Terminal(argv, env, cwd)
            terms.append(t)
            procs.append(t.proc)
            # Allow initial screen/capability negotiation before injecting input.
            deadline = time.perf_counter() + 15
            prompt_marker = "1>" if tool == "jcode" else "❯"
            while prompt_marker not in t.text() and time.perf_counter() < deadline:
                pump(terms, 0.002)
                if t.proc.poll() is not None:
                    raise RuntimeError(t.text())
            if prompt_marker not in t.text():
                raise RuntimeError(f"Chat prompt did not appear: {t.text()}")
            latency = echo(terms, t, f"bench{i}x")
            startup.append(dict(first_visible_ms=t.visible_ms, input_ready_ms=(time.perf_counter() - t.start) * 1000, initial_echo_ms=latency))
        first_screen = terms[0].text()
        pump(terms, 2)
        roots = [p.pid for p in procs]
        before = snapshot(roots)
        bytes_before = sum(t.bytes for t in terms)
        start_idle = time.perf_counter()
        samples = []
        for _ in range(max(1, int(idle_seconds * 5))):
            pump(terms, 0.2)
            samples.append(snapshot(roots))
        wall = time.perf_counter() - start_idle
        after = snapshot(roots)
        idle_bytes = sum(t.bytes for t in terms) - bytes_before
        latencies = []
        for i in range(20):
            t = terms[i % len(terms)]
            latencies.append(echo(terms, t, f"q{i:02}z"))
        return dict(tool=tool, sessions=sessions, server_ready_ms=server_ms, cold_input_ready_ms=server_ms + startup[0]["input_ready_ms"], startup=startup, idle_seconds=wall, idle_cpu_percent_one_core=100 * (after["cpu_seconds"] - before["cpu_seconds"]) / wall, idle_output_bytes=idle_bytes, pss_mib=statistics.median(x["pss_mib"] for x in samples), private_mib=statistics.median(x["private_mib"] for x in samples), rss_mib=statistics.median(x["rss_mib"] for x in samples), processes=after["processes"], input_echo_median_ms=statistics.median(latencies), input_echo_max_ms=max(latencies), input_echo_samples_ms=latencies, screen=first_screen, settled_screen=terms[0].text(), executables={str(p.pid): os.readlink(f"/proc/{p.pid}/exe") for p in procs})
    finally:
        for p in reversed(procs):
            stop(p)
        for t in terms:
            os.close(t.fd)
        shutil.rmtree(work)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--jcode", default=shutil.which("jcode"))
    p.add_argument("--maki", required=True)
    p.add_argument("--tools", nargs="+", choices=["jcode", "maki"], default=["jcode", "maki"])
    p.add_argument("--maki-no-splash", action="store_true")
    p.add_argument("--runs", type=int, default=3)
    p.add_argument("--sessions", nargs="+", type=int, default=[1, 5, 10])
    p.add_argument("--idle-seconds", type=float, default=5)
    p.add_argument("--output", required=True)
    args = p.parse_args()
    out = Path(args.output).resolve()
    out.parent.mkdir(parents=True, exist_ok=True)
    results = dict(method="Fresh process, warm filesystem cache, isolated empty workspace and HOME, dummy API key, no model requests. 120x40 PTY. Jcode memory disabled, dedicated server included. Maki user plugins disabled, builtins enabled. No prompts submitted. CPU is percentage of one logical core, /proc clock-tick resolution. Startup includes Python spawn and terminal parsing overhead. Server readiness polls at 2ms. Jcode returning-user marker seeded to skip onboarding. PSS depends on sharing with unrelated running processes, private memory is also reported.", timestamp=time.strftime("%Y-%m-%dT%H:%M:%S%z"), host=dict(uname=list(os.uname()), cpu_count=os.cpu_count(), loadavg=os.getloadavg()), binaries={}, runs=[])
    binaries = {"jcode": str(Path(args.jcode).resolve()), "maki": str(Path(args.maki).resolve())}
    results["maki_no_splash"] = args.maki_no_splash
    binaries = {name: path for name, path in binaries.items() if name in args.tools}
    for tool, binary in binaries.items():
        version = subprocess.check_output([binary, "--version"], text=True).strip()
        results["binaries"][tool] = dict(path=binary, version=version, bytes=Path(binary).stat().st_size, sha256=hashlib.file_digest(open(binary, "rb"), "sha256").hexdigest())
    jobs = [(tool, n, r) for r in range(args.runs) for n in args.sessions for tool in binaries]
    random.Random(42).shuffle(jobs)
    for tool, n, r in jobs:
        print(f"{tool}: {n} sessions, repetition {r + 1}", flush=True)
        result = run(tool, binaries[tool], n, args.idle_seconds, out.parent, args.maki_no_splash)
        result["repetition"] = r + 1
        results["runs"].append(result)
        out.write_text(json.dumps(results, indent=2))
        print(json.dumps({k: v for k, v in result.items() if k not in ("screen", "settled_screen", "startup", "input_echo_samples_ms")}), flush=True)
    print(f"Saved {out}", flush=True)


if __name__ == "__main__":
    main()
