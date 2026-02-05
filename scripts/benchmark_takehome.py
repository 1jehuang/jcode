#!/usr/bin/env python3
"""
Benchmark single agent vs swarm on the Anthropic Performance Take-Home.

Usage:
    BENCHMARK_TIMEOUT=5 python scripts/benchmark_takehome.py single
    BENCHMARK_TIMEOUT=10 python scripts/benchmark_takehome.py swarm
    BENCHMARK_TIMEOUT=10 python scripts/benchmark_takehome.py both
"""

import socket
import json
import os
import sys
import time
import select
import shutil
import subprocess
import threading

DEBUG_SOCKET = f"/run/user/{os.getuid()}/jcode-debug.sock"
TAKEHOME_SOURCE = "/home/jeremy/original_performance_takehome"
BENCHMARK_DIR = "/tmp/takehome-benchmark"
TIMEOUT_MINUTES = int(os.environ.get('BENCHMARK_TIMEOUT', '10'))
BASELINE = 147734


def send_cmd(cmd: str, session_id: str = None, timeout: float = 300) -> tuple:
    """Send a debug command and get response."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(DEBUG_SOCKET)
    sock.setblocking(False)

    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id

    sock.send((json.dumps(req) + '\n').encode())

    start = time.time()
    data = b""
    while time.time() - start < timeout:
        ready, _, _ = select.select([sock], [], [], 1.0)
        if ready:
            try:
                chunk = sock.recv(65536)
                if not chunk:
                    break
                data += chunk
                if b'\n' in data:
                    break
            except BlockingIOError:
                continue

    sock.close()

    if not data:
        return False, "", "Timeout"

    try:
        resp = json.loads(data.decode().strip())
        return resp.get('ok', False), resp.get('output', ''), resp.get('error', '')
    except json.JSONDecodeError as e:
        return False, "", f"JSON error: {e}"


def create_session(working_dir: str) -> tuple:
    """Create a session and return (session_id, friendly_name)."""
    ok, output, err = send_cmd(f"create_session:{working_dir}", timeout=120)
    if not ok:
        raise RuntimeError(f"Failed to create session: {err}")
    data = json.loads(output)
    return data['session_id'], data.get('friendly_name', data['session_id'][:12])


def destroy_session(session_id: str):
    """Destroy a session."""
    send_cmd(f"destroy_session:{session_id}")


def setup_workspace(name: str) -> str:
    """Create a clean copy of the take-home."""
    workspace = os.path.join(BENCHMARK_DIR, name)
    if os.path.exists(workspace):
        shutil.rmtree(workspace)
    shutil.copytree(TAKEHOME_SOURCE, workspace)
    return workspace


def get_cycles(workspace: str) -> int:
    """Run tests and return cycle count."""
    try:
        result = subprocess.run(
            ["python", "tests/submission_tests.py", "-v"],
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=120
        )
        for line in (result.stdout + result.stderr).split('\n'):
            if 'CYCLES:' in line:
                return int(line.split('CYCLES:')[1].strip())
    except Exception as e:
        print(f"Error getting cycles: {e}")
    return BASELINE


def make_single_prompt(workspace: str) -> str:
    return f"""You are optimizing a VLIW SIMD kernel for Anthropic's performance take-home.

IMPORTANT: You MUST work in this directory: {workspace}
All file paths should be relative to or within this directory.

Goal: Reduce the cycle count from 147,734 to as low as possible.

Key files (in {workspace}):
- problem.py: Defines the Machine, instruction set (VLIW bundles, vector ops, VLEN=16)
- perf_takehome.py: Contains KernelBuilder.build_kernel() - this is what you optimize
- tests/submission_tests.py: Run to verify correctness and see cycle count

DO NOT modify tests/ folder.

Key optimizations to try:
1. VLIW parallelism - pack independent operations into single bundles
2. Vector operations - use VLEN=16 to process 16 elements at once
3. Reduce memory access latency - batch loads/stores
4. Optimize the hash function - it runs many times per element

Start by reading {workspace}/problem.py to understand the machine, then optimize build_kernel().
After each change, run `cd {workspace} && python tests/submission_tests.py` to check correctness and cycles.

Work efficiently - focus on the highest-impact optimizations first."""


def run_single_agent() -> dict:
    """Run a single agent benchmark using async messaging."""
    print("\n" + "=" * 60)
    print(f"SINGLE AGENT BENCHMARK (timeout: {TIMEOUT_MINUTES}m)")
    print("=" * 60)

    workspace = setup_workspace("single")
    print(f"Workspace: {workspace}")

    start_time = time.time()
    session_id = None
    best_cycles = BASELINE

    try:
        session_id, name = create_session(workspace)
        print(f"Session: {name}")

        # Initial cycles
        cycles = get_cycles(workspace)
        print(f"Baseline: {cycles} cycles")

        # Send optimization task asynchronously
        print("\nStarting optimization (async)...")
        prompt = make_single_prompt(workspace)

        # Use message_async to start the job
        ok, output, err = send_cmd(f"message_async:{prompt}", session_id, timeout=30)
        if not ok:
            print(f"Failed to start async job: {err}")
            return {"approach": "single", "cycles": BASELINE, "time_seconds": 0, "error": err}

        job_data = json.loads(output)
        job_id = job_data.get("job_id")
        print(f"Job started: {job_id}")

        timeout_seconds = TIMEOUT_MINUTES * 60
        last_cycles = BASELINE
        check_interval = 30  # Check every 30 seconds

        while time.time() - start_time < timeout_seconds:
            elapsed = time.time() - start_time

            # Check job status
            ok, status_output, _ = send_cmd(f"job_status:{job_id}", session_id, timeout=10)
            if ok:
                try:
                    status = json.loads(status_output)
                    job_status = status.get("status", "unknown")
                    if job_status in ["completed", "failed"]:
                        print(f"\n[{elapsed/60:.1f}m] Job {job_status}")
                        break
                except:
                    pass

            # Check current cycles in workspace
            cycles = get_cycles(workspace)
            if cycles < best_cycles:
                best_cycles = cycles
                print(f"[{elapsed/60:.1f}m] NEW BEST: {cycles} cycles ({BASELINE/cycles:.2f}x speedup)")
            elif cycles != last_cycles:
                print(f"[{elapsed/60:.1f}m] Cycles: {cycles}")
            last_cycles = cycles

            time.sleep(check_interval)

        # Final check
        cycles = get_cycles(workspace)
        if cycles < best_cycles:
            best_cycles = cycles

        elapsed = time.time() - start_time
        print(f"\nFinal: {best_cycles} cycles in {elapsed/60:.1f}m ({BASELINE/best_cycles:.2f}x)")

        return {
            "approach": "single",
            "cycles": best_cycles,
            "time_seconds": elapsed,
            "workspace": workspace
        }

    except Exception as e:
        print(f"Error: {e}")
        return {
            "approach": "single",
            "cycles": BASELINE,
            "time_seconds": time.time() - start_time,
            "error": str(e)
        }
    finally:
        if session_id:
            destroy_session(session_id)


def run_swarm(n_agents: int = 2) -> dict:
    """Run autonomous swarm benchmark using swarm_message_async.

    This uses the full swarm capability where ONE agent becomes coordinator,
    creates a plan, and spawns subagents automatically.
    """
    print("\n" + "=" * 60)
    print(f"AUTONOMOUS SWARM BENCHMARK (timeout: {TIMEOUT_MINUTES}m)")
    print("=" * 60)

    workspace = setup_workspace("swarm")
    print(f"Workspace: {workspace}")

    start_time = time.time()
    session_id = None
    best_cycles = BASELINE

    try:
        # Create ONE session - it becomes coordinator and spawns agents
        session_id, name = create_session(workspace)
        print(f"Coordinator: {name}")

        baseline = get_cycles(workspace)
        print(f"Baseline: {baseline} cycles")

        # Use swarm_message_async - this will:
        # 1. Plan subtasks automatically
        # 2. Spawn subagents to work in parallel
        # 3. Integrate results
        prompt = f"""Optimize the VLIW SIMD kernel in {workspace}/perf_takehome.py to minimize cycle count.

Current baseline: 147,734 cycles. Goal: as low as possible.

The problem:
- {workspace}/problem.py defines the machine (VLEN=16 vectors, VLIW bundles, slot limits)
- {workspace}/perf_takehome.py has build_kernel() which needs optimization
- Run `cd {workspace} && python tests/submission_tests.py` to verify correctness and check cycles

Key optimization strategies:
1. Vectorization - use VLEN=16 to process 16 elements at once
2. VLIW packing - bundle independent operations together
3. Reduce memory latency - batch loads/stores
4. Optimize hash function - it runs many times per element

Break this into parallel subtasks and spawn agents to work on different optimizations.
DO NOT modify tests/ folder."""

        print("\nStarting autonomous swarm (swarm_message_async)...")
        ok, output, err = send_cmd(f"swarm_message_async:{prompt}", session_id, timeout=30)
        if not ok:
            print(f"Failed to start swarm: {err}")
            return {"approach": "swarm", "cycles": BASELINE, "time_seconds": 0, "error": err}

        job_data = json.loads(output)
        job_id = job_data.get("job_id")
        print(f"Swarm job started: {job_id}")

        timeout_seconds = TIMEOUT_MINUTES * 60
        last_cycles = BASELINE
        check_interval = 30

        while time.time() - start_time < timeout_seconds:
            elapsed = time.time() - start_time

            # Check job status
            ok, status_output, _ = send_cmd(f"job_status:{job_id}", session_id, timeout=10)
            if ok:
                try:
                    status = json.loads(status_output)
                    job_status = status.get("status", "unknown")
                    if job_status == "completed":
                        print(f"\n[{elapsed/60:.1f}m] Swarm completed!")
                        break
                    elif job_status == "failed":
                        print(f"\n[{elapsed/60:.1f}m] Swarm failed: {status.get('error', 'unknown')}")
                        break
                except:
                    pass

            # Check swarm members (to see how many agents were spawned)
            ok, swarm_output, _ = send_cmd("swarm:members", session_id, timeout=10)
            if ok and elapsed < 60:  # Only print once early on
                try:
                    print(f"[{elapsed/60:.1f}m] Swarm: {swarm_output[:100]}...")
                except:
                    pass

            # Check current cycles
            cycles = get_cycles(workspace)
            if cycles < best_cycles:
                best_cycles = cycles
                print(f"[{elapsed/60:.1f}m] NEW BEST: {cycles} cycles ({BASELINE/cycles:.2f}x)")
            elif cycles != last_cycles:
                print(f"[{elapsed/60:.1f}m] Cycles: {cycles}")
            last_cycles = cycles

            time.sleep(check_interval)

        # Final check
        cycles = get_cycles(workspace)
        if cycles < best_cycles:
            best_cycles = cycles

        elapsed = time.time() - start_time
        print(f"\nFinal: {best_cycles} cycles in {elapsed/60:.1f}m ({BASELINE/best_cycles:.2f}x)")

        return {
            "approach": "swarm",
            "cycles": best_cycles,
            "time_seconds": elapsed,
            "workspace": workspace
        }

    except Exception as e:
        print(f"Error: {e}")
        return {
            "approach": "swarm",
            "cycles": BASELINE,
            "time_seconds": time.time() - start_time,
            "error": str(e)
        }
    finally:
        if session_id:
            destroy_session(session_id)


def print_results(results: dict):
    """Print comparison table."""
    print("\n" + "=" * 60)
    print("RESULTS")
    print("=" * 60)
    print(f"{'Approach':<15} {'Cycles':<12} {'Time':<10} {'Speedup':<10}")
    print("-" * 60)

    for name, data in results.items():
        cycles = data['cycles']
        time_m = data['time_seconds'] / 60
        speedup = BASELINE / cycles
        print(f"{name:<15} {cycles:<12} {time_m:<10.1f}m {speedup:<10.2f}x")

    if len(results) > 1:
        winner = min(results.items(), key=lambda x: x[1]['cycles'])
        print(f"\nWinner: {winner[0]} ({winner[1]['cycles']} cycles)")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    mode = sys.argv[1].lower()
    os.makedirs(BENCHMARK_DIR, exist_ok=True)

    print(f"Benchmark timeout: {TIMEOUT_MINUTES} minutes per approach")
    print(f"Set BENCHMARK_TIMEOUT env var to change (e.g., BENCHMARK_TIMEOUT=30)")

    if mode == "single":
        r = run_single_agent()
        print_results({"single": r})

    elif mode == "swarm":
        r = run_swarm()
        print_results({"swarm": r})

    elif mode == "both":
        results = {}
        results['single'] = run_single_agent()
        results['swarm'] = run_swarm()
        print_results(results)

    else:
        print(f"Unknown mode: {mode}")
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
