#!/usr/bin/env python3
"""Benchmark jcode startup time."""

import subprocess
import time
import statistics
import sys
import os
import signal

def measure_help_startup(binary: str, runs: int = 10) -> list[float]:
    """Measure --help startup time (baseline binary load)."""
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run([binary, "--help"], capture_output=True)
        elapsed = time.perf_counter() - start
        times.append(elapsed * 1000)  # ms
    return times

def measure_version_startup(binary: str, runs: int = 10) -> list[float]:
    """Measure --version startup time."""
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run([binary, "--version"], capture_output=True)
        elapsed = time.perf_counter() - start
        times.append(elapsed * 1000)
    return times

def kill_server():
    """Kill any running jcode server."""
    subprocess.run(["pkill", "-f", "jcode serve"], capture_output=True)
    time.sleep(0.2)
    socket_path = f"/run/user/{os.getuid()}/jcode.sock"
    debug_socket = f"/run/user/{os.getuid()}/jcode-debug.sock"
    for s in [socket_path, debug_socket]:
        try:
            os.remove(s)
        except FileNotFoundError:
            pass

def measure_server_startup(binary: str, runs: int = 5) -> list[float]:
    """Measure time for server to become ready (socket exists and responds)."""
    socket_path = f"/run/user/{os.getuid()}/jcode.sock"
    times = []

    for i in range(runs):
        kill_server()

        start = time.perf_counter()

        # Start server in background
        proc = subprocess.Popen(
            [binary, "serve"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        # Wait for socket to exist and be connectable
        import socket
        ready = False
        while time.perf_counter() - start < 10:  # 10s timeout
            if os.path.exists(socket_path):
                try:
                    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    sock.connect(socket_path)
                    sock.close()
                    ready = True
                    break
                except (ConnectionRefusedError, FileNotFoundError):
                    pass
            time.sleep(0.01)

        elapsed = time.perf_counter() - start

        if ready:
            times.append(elapsed * 1000)
        else:
            print(f"  Warning: server didn't start in time (run {i+1})")

        # Clean up
        proc.terminate()
        proc.wait()

    kill_server()
    return times

def measure_server_startup_no_update(binary: str, runs: int = 5) -> list[float]:
    """Measure server startup with --no-update flag."""
    socket_path = f"/run/user/{os.getuid()}/jcode.sock"
    times = []

    for i in range(runs):
        kill_server()

        # Set env to skip update check
        env = os.environ.copy()

        start = time.perf_counter()

        # Start server in background with no update check
        # Note: serve command doesn't support --no-update directly,
        # but background update check means it won't block
        proc = subprocess.Popen(
            [binary, "serve"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=env,
        )

        # Wait for socket
        import socket
        ready = False
        while time.perf_counter() - start < 10:
            if os.path.exists(socket_path):
                try:
                    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    sock.connect(socket_path)
                    sock.close()
                    ready = True
                    break
                except (ConnectionRefusedError, FileNotFoundError):
                    pass
            time.sleep(0.01)

        elapsed = time.perf_counter() - start

        if ready:
            times.append(elapsed * 1000)
        else:
            print(f"  Warning: server didn't start in time (run {i+1})")

        proc.terminate()
        proc.wait()

    kill_server()
    return times

def print_stats(name: str, times: list[float]):
    """Print timing statistics."""
    if not times:
        print(f"\n{name}: No successful runs")
        return

    print(f"\n{name}:")
    print(f"  Min:    {min(times):.2f} ms")
    print(f"  Max:    {max(times):.2f} ms")
    print(f"  Mean:   {statistics.mean(times):.2f} ms")
    print(f"  Median: {statistics.median(times):.2f} ms")
    if len(times) > 1:
        print(f"  Stdev:  {statistics.stdev(times):.2f} ms")

def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "./target/release/jcode"

    if not os.path.exists(binary):
        print(f"Binary not found: {binary}")
        print("Run: cargo build --release")
        sys.exit(1)

    print(f"Benchmarking: {binary}")
    print("=" * 50)

    # Warm up filesystem cache
    subprocess.run([binary, "--version"], capture_output=True)

    # Quick benchmarks
    help_times = measure_help_startup(binary)
    print_stats("--help (binary load)", help_times)

    version_times = measure_version_startup(binary)
    print_stats("--version", version_times)

    # Server startup benchmark
    print("\nMeasuring server startup (5 runs each)...")
    server_times = measure_server_startup(binary)
    print_stats("Server ready (socket connectable)", server_times)

    print("\n" + "=" * 50)
    print("Summary:")
    print(f"  Binary load:    ~{statistics.median(help_times):.1f} ms")
    if server_times:
        print(f"  Server ready:   ~{statistics.median(server_times):.1f} ms")

if __name__ == "__main__":
    main()
