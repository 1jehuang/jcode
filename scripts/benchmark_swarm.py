#!/usr/bin/env python3
"""
Benchmark: single agent vs swarm on the Anthropic Performance Take-Home.

Compares jcode's swarm (multi-agent coordination) with single-agent performance
on the VLIW SIMD kernel optimization challenge.

Usage:
    python scripts/benchmark_swarm.py                  # Run both trials
    python scripts/benchmark_swarm.py --single-only    # Single agent only
    python scripts/benchmark_swarm.py --swarm-only     # Swarm only
    python scripts/benchmark_swarm.py --timeout 30     # 30 minute timeout per trial
    python scripts/benchmark_swarm.py --check-interval 15  # Check cycles every 15s

Environment:
    Requires jcode server running with debug_control enabled:
        touch ~/.jcode/debug_control
        jcode serve
"""

import argparse
import json
import os
import select
import shutil
import socket
import subprocess
import sys
import time

DEBUG_SOCKET = f"/run/user/{os.getuid()}/jcode-debug.sock"
MAIN_SOCKET = f"/run/user/{os.getuid()}/jcode.sock"
TAKEHOME_SOURCE = "/home/jeremy/original_performance_takehome"
BENCHMARK_DIR = "/tmp/takehome-benchmark"
BASELINE = 147734


# ---------------------------------------------------------------------------
# Socket helpers
# ---------------------------------------------------------------------------

def send_cmd(cmd: str, session_id: str = None, timeout: float = 300) -> tuple:
    """Send a debug command and return (ok, output, error)."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(DEBUG_SOCKET)
    sock.setblocking(False)

    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id

    sock.send((json.dumps(req) + "\n").encode())

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
                if b"\n" in data:
                    break
            except BlockingIOError:
                continue

    sock.close()

    if not data:
        return False, "", "Timeout"

    try:
        resp = json.loads(data.decode().strip())
        return resp.get("ok", False), resp.get("output", ""), resp.get("error", "")
    except json.JSONDecodeError as e:
        return False, "", f"JSON error: {e}"


def create_session(working_dir: str) -> tuple:
    """Create a headless session. Returns (session_id, friendly_name)."""
    ok, output, err = send_cmd(f"create_session:{working_dir}", timeout=120)
    if not ok:
        raise RuntimeError(f"Failed to create session: {err}")
    data = json.loads(output)
    return data["session_id"], data.get("friendly_name", data["session_id"][:12])


def destroy_session(session_id: str):
    """Destroy a session."""
    send_cmd(f"destroy_session:{session_id}")


# ---------------------------------------------------------------------------
# Workspace helpers
# ---------------------------------------------------------------------------

def setup_workspace(name: str) -> str:
    """Create a clean copy of the take-home challenge."""
    workspace = os.path.join(BENCHMARK_DIR, name)
    if os.path.exists(workspace):
        shutil.rmtree(workspace)
    shutil.copytree(TAKEHOME_SOURCE, workspace)
    # Initialize a git repo so swarm_id detection works
    subprocess.run(["git", "init"], cwd=workspace, capture_output=True)
    subprocess.run(["git", "add", "."], cwd=workspace, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "initial"],
        cwd=workspace,
        capture_output=True,
        env={**os.environ, "GIT_AUTHOR_NAME": "bench", "GIT_AUTHOR_EMAIL": "b@b",
             "GIT_COMMITTER_NAME": "bench", "GIT_COMMITTER_EMAIL": "b@b"},
    )
    return workspace


def get_cycles(workspace: str) -> int:
    """Run submission_tests.py and extract cycle count."""
    try:
        result = subprocess.run(
            [sys.executable, "tests/submission_tests.py", "-v"],
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=120,
        )
        for line in (result.stdout + result.stderr).split("\n"):
            if "CYCLES:" in line:
                return int(line.split("CYCLES:")[1].strip())
    except Exception as e:
        print(f"  Error getting cycles: {e}")
    return BASELINE


def get_test_summary(workspace: str) -> str:
    """Run submission tests and return the full output summary."""
    try:
        result = subprocess.run(
            [sys.executable, "tests/submission_tests.py", "-v"],
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=120,
        )
        return result.stdout + result.stderr
    except Exception as e:
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# Optimization prompt
# ---------------------------------------------------------------------------

OPTIMIZATION_PROMPT_TEMPLATE = """Optimize the build_kernel() method in perf_takehome.py to minimize cycle count \
on the VLIW SIMD machine simulator. The baseline is 147,734 cycles.

IMPORTANT: You MUST work in this directory: {workspace}
All file paths should be relative to or within this directory.

Key files (in {workspace}):
- problem.py: Defines the Machine, instruction set, slot limits, engines
- perf_takehome.py: Contains KernelBuilder.build_kernel() - THIS is what you optimize
- tests/submission_tests.py: Run to verify correctness and see cycle count. DO NOT modify tests/.

Machine details (read problem.py for full spec):
- VLEN=8 vector width, N_CORES=1
- VLIW bundles: multiple operations per cycle, subject to slot limits per engine
- Engines: load, store, alu, flow, debug, vload, vstore, valu
- Scratch memory for temporaries (SCRATCH_SIZE limit)

Focus on these optimization strategies:
1. Vectorization - use VALU/VLOAD/VSTORE engines with VLEN=8 to process 8 elements at once
2. VLIW instruction packing - bundle independent operations into the same cycle
3. Loop structure - unroll loops, reduce iteration overhead
4. Hash function optimization - it runs many times; pack hash stages
5. Efficient memory access patterns - batch loads/stores, reduce address computation

After each change, verify with: cd {workspace} && python tests/submission_tests.py

Work efficiently - focus on the highest-impact optimizations first."""


# ---------------------------------------------------------------------------
# Poll loop for async jobs
# ---------------------------------------------------------------------------

def poll_job(
    job_id: str,
    session_id: str,
    workspace: str,
    start_time: float,
    timeout_seconds: float,
    check_interval: float,
    label: str,
) -> int:
    """Poll a job until completion, printing cycle updates. Returns best cycle count."""
    best_cycles = BASELINE
    last_cycles = BASELINE

    while time.time() - start_time < timeout_seconds:
        elapsed = time.time() - start_time

        # Check job status
        ok, status_output, _ = send_cmd(f"job_status:{job_id}", session_id, timeout=10)
        if ok:
            try:
                status = json.loads(status_output)
                job_status = status.get("status", "unknown")
                if job_status == "completed":
                    print(f"\n  [{label}] [{elapsed/60:.1f}m] Job completed")
                    break
                elif job_status == "failed":
                    error = status.get("error", "unknown")
                    print(f"\n  [{label}] [{elapsed/60:.1f}m] Job failed: {error}")
                    break
            except (json.JSONDecodeError, ValueError):
                pass

        # Check current cycles in workspace
        cycles = get_cycles(workspace)
        if cycles < best_cycles:
            best_cycles = cycles
            speedup = BASELINE / cycles
            print(f"  [{label}] [{elapsed/60:.1f}m] NEW BEST: {cycles} cycles ({speedup:.2f}x speedup)")
        elif cycles != last_cycles:
            print(f"  [{label}] [{elapsed/60:.1f}m] Cycles: {cycles}")
        last_cycles = cycles

        time.sleep(check_interval)

    # Final check
    cycles = get_cycles(workspace)
    if cycles < best_cycles:
        best_cycles = cycles

    return best_cycles


# ---------------------------------------------------------------------------
# Trial A: Single Agent
# ---------------------------------------------------------------------------

def run_single_agent(timeout_minutes: float, check_interval: float) -> dict:
    """Run a single agent on the optimization task."""
    print("\n" + "=" * 70)
    print(f"  TRIAL A: SINGLE AGENT (timeout: {timeout_minutes}m)")
    print("=" * 70)

    workspace = setup_workspace("single")
    print(f"  Workspace: {workspace}")

    start_time = time.time()
    session_id = None

    try:
        session_id, name = create_session(workspace)
        print(f"  Session: {name} ({session_id[:12]}...)")

        baseline_cycles = get_cycles(workspace)
        print(f"  Baseline: {baseline_cycles} cycles")

        # Build prompt
        prompt = OPTIMIZATION_PROMPT_TEMPLATE.format(workspace=workspace)

        # Start async job
        print("\n  Starting optimization (message_async)...")
        ok, output, err = send_cmd(f"message_async:{prompt}", session_id, timeout=30)
        if not ok:
            print(f"  Failed to start async job: {err}")
            return {
                "approach": "single",
                "cycles": BASELINE,
                "time_seconds": 0,
                "error": err,
            }

        job_data = json.loads(output)
        job_id = job_data.get("job_id")
        print(f"  Job started: {job_id}")

        # Poll until done
        timeout_seconds = timeout_minutes * 60
        best_cycles = poll_job(
            job_id, session_id, workspace, start_time, timeout_seconds,
            check_interval, "single",
        )

        elapsed = time.time() - start_time
        speedup = BASELINE / best_cycles if best_cycles > 0 else 0
        print(f"\n  SINGLE AGENT RESULT: {best_cycles} cycles in {elapsed/60:.1f}m ({speedup:.2f}x)")

        # Get full test output
        test_output = get_test_summary(workspace)
        print(f"\n  Test output:\n{test_output}")

        return {
            "approach": "single",
            "cycles": best_cycles,
            "time_seconds": elapsed,
            "workspace": workspace,
        }

    except Exception as e:
        elapsed = time.time() - start_time
        print(f"  Error: {e}")
        return {
            "approach": "single",
            "cycles": BASELINE,
            "time_seconds": elapsed,
            "error": str(e),
        }
    finally:
        if session_id:
            print(f"  Cleaning up session {session_id[:12]}...")
            destroy_session(session_id)


# ---------------------------------------------------------------------------
# Trial B: Swarm (Multi-Agent)
# ---------------------------------------------------------------------------

def run_swarm(timeout_minutes: float, check_interval: float) -> dict:
    """Run swarm multi-agent on the optimization task."""
    print("\n" + "=" * 70)
    print(f"  TRIAL B: SWARM / MULTI-AGENT (timeout: {timeout_minutes}m)")
    print("=" * 70)

    workspace = setup_workspace("swarm")
    print(f"  Workspace: {workspace}")

    start_time = time.time()
    session_id = None

    try:
        session_id, name = create_session(workspace)
        print(f"  Coordinator: {name} ({session_id[:12]}...)")

        baseline_cycles = get_cycles(workspace)
        print(f"  Baseline: {baseline_cycles} cycles")

        # Build prompt (same optimization goal)
        prompt = OPTIMIZATION_PROMPT_TEMPLATE.format(workspace=workspace)

        # Start swarm async job - this automatically plans subtasks and spawns agents
        print("\n  Starting swarm (swarm_message_async)...")
        ok, output, err = send_cmd(f"swarm_message_async:{prompt}", session_id, timeout=30)
        if not ok:
            print(f"  Failed to start swarm: {err}")
            return {
                "approach": "swarm",
                "cycles": BASELINE,
                "time_seconds": 0,
                "error": err,
            }

        job_data = json.loads(output)
        job_id = job_data.get("job_id")
        print(f"  Swarm job started: {job_id}")

        timeout_seconds = timeout_minutes * 60
        best_cycles = BASELINE
        last_cycles = BASELINE
        member_info_printed = False

        while time.time() - start_time < timeout_seconds:
            elapsed = time.time() - start_time

            # Check job status
            ok, status_output, _ = send_cmd(f"job_status:{job_id}", session_id, timeout=10)
            if ok:
                try:
                    status = json.loads(status_output)
                    job_status = status.get("status", "unknown")
                    if job_status == "completed":
                        print(f"\n  [swarm] [{elapsed/60:.1f}m] Swarm completed!")
                        break
                    elif job_status == "failed":
                        error = status.get("error", "unknown")
                        print(f"\n  [swarm] [{elapsed/60:.1f}m] Swarm failed: {error}")
                        break
                except (json.JSONDecodeError, ValueError):
                    pass

            # Show swarm members (once, early on)
            if not member_info_printed and elapsed > 10:
                ok, swarm_output, _ = send_cmd("swarm:members", session_id, timeout=10)
                if ok:
                    try:
                        members = json.loads(swarm_output)
                        print(f"  [swarm] [{elapsed/60:.1f}m] {len(members)} agent(s) in swarm")
                        for m in members[:5]:
                            sid = m.get("session_id", "?")[:12]
                            st = m.get("status", "?")
                            print(f"    - {sid}... ({st})")
                        member_info_printed = True
                    except (json.JSONDecodeError, ValueError):
                        pass

            # Check current cycles
            cycles = get_cycles(workspace)
            if cycles < best_cycles:
                best_cycles = cycles
                speedup = BASELINE / cycles
                print(f"  [swarm] [{elapsed/60:.1f}m] NEW BEST: {cycles} cycles ({speedup:.2f}x speedup)")
            elif cycles != last_cycles:
                print(f"  [swarm] [{elapsed/60:.1f}m] Cycles: {cycles}")
            last_cycles = cycles

            time.sleep(check_interval)

        # Final check
        cycles = get_cycles(workspace)
        if cycles < best_cycles:
            best_cycles = cycles

        elapsed = time.time() - start_time
        speedup = BASELINE / best_cycles if best_cycles > 0 else 0
        print(f"\n  SWARM RESULT: {best_cycles} cycles in {elapsed/60:.1f}m ({speedup:.2f}x)")

        # Get full test output
        test_output = get_test_summary(workspace)
        print(f"\n  Test output:\n{test_output}")

        return {
            "approach": "swarm",
            "cycles": best_cycles,
            "time_seconds": elapsed,
            "workspace": workspace,
        }

    except Exception as e:
        elapsed = time.time() - start_time
        print(f"  Error: {e}")
        return {
            "approach": "swarm",
            "cycles": BASELINE,
            "time_seconds": elapsed,
            "error": str(e),
        }
    finally:
        if session_id:
            print(f"  Cleaning up session {session_id[:12]}...")
            destroy_session(session_id)


# ---------------------------------------------------------------------------
# Results comparison
# ---------------------------------------------------------------------------

def print_comparison(results: dict):
    """Print a comparison table of all trials."""
    print("\n" + "=" * 70)
    print("  BENCHMARK RESULTS")
    print("=" * 70)

    header = f"  {'Approach':<15} {'Cycles':<12} {'Time':<12} {'Speedup':<12} {'Status'}"
    print(header)
    print("  " + "-" * 66)

    for name, data in results.items():
        cycles = data["cycles"]
        time_m = data["time_seconds"] / 60
        speedup = BASELINE / cycles if cycles > 0 else 0
        status = "ERROR" if "error" in data else "OK"
        print(f"  {name:<15} {cycles:<12} {time_m:<12.1f}m {speedup:<12.2f}x {status}")

    if len(results) > 1:
        print()
        winner = min(results.items(), key=lambda x: x[1]["cycles"])
        loser = max(results.items(), key=lambda x: x[1]["cycles"])

        winner_name, winner_data = winner
        loser_name, loser_data = loser

        print(f"  Winner: {winner_name} ({winner_data['cycles']} cycles)")
        if loser_data["cycles"] > 0 and winner_data["cycles"] > 0:
            relative = loser_data["cycles"] / winner_data["cycles"]
            print(f"  {winner_name} is {relative:.2f}x better than {loser_name}")

        # Time comparison
        if winner_data["time_seconds"] > 0 and loser_data["time_seconds"] > 0:
            time_ratio = loser_data["time_seconds"] / winner_data["time_seconds"]
            if time_ratio > 1:
                print(f"  {winner_name} was {time_ratio:.1f}x faster in wall time")
            else:
                print(f"  {loser_name} was {1/time_ratio:.1f}x faster in wall time")

    # Threshold analysis
    print("\n  Threshold Analysis:")
    thresholds = [
        ("Baseline", BASELINE),
        ("Updated starter", 18532),
        ("Opus 4 many hours", 2164),
        ("Opus 4.5 casual (best human 2hr)", 1790),
        ("Opus 4.5 2hr harness", 1579),
        ("Sonnet 4.5 many hours", 1548),
        ("Opus 4.5 11.5hr harness", 1487),
        ("Opus 4.5 improved harness", 1363),
    ]

    for name, data in results.items():
        cycles = data["cycles"]
        print(f"\n  {name} ({cycles} cycles):")
        for thresh_name, thresh_val in thresholds:
            passed = "PASS" if cycles < thresh_val else "FAIL"
            print(f"    [{passed}] {thresh_name}: < {thresh_val}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Benchmark single agent vs swarm on VLIW SIMD optimization task",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--timeout", type=float, default=10,
        help="Timeout in minutes per trial (default: 10)",
    )
    parser.add_argument(
        "--check-interval", type=float, default=30,
        help="How often to check cycle count, in seconds (default: 30)",
    )
    parser.add_argument(
        "--single-only", action="store_true",
        help="Only run single agent trial",
    )
    parser.add_argument(
        "--swarm-only", action="store_true",
        help="Only run swarm trial",
    )
    args = parser.parse_args()

    # Validate environment
    if not os.path.exists(DEBUG_SOCKET):
        print(f"Error: Debug socket not found: {DEBUG_SOCKET}")
        print("Make sure jcode server is running with debug_control enabled:")
        print("  touch ~/.jcode/debug_control")
        print("  jcode serve")
        sys.exit(1)

    if not os.path.exists(TAKEHOME_SOURCE):
        print(f"Error: Take-home source not found: {TAKEHOME_SOURCE}")
        sys.exit(1)

    os.makedirs(BENCHMARK_DIR, exist_ok=True)

    print("=" * 70)
    print("  SWARM vs SINGLE-AGENT BENCHMARK")
    print("=" * 70)
    print(f"  Timeout:        {args.timeout} minutes per trial")
    print(f"  Check interval: {args.check_interval} seconds")
    print(f"  Source:         {TAKEHOME_SOURCE}")
    print(f"  Baseline:       {BASELINE} cycles")
    print()

    results = {}

    run_single = not args.swarm_only
    run_multi = not args.single_only

    if run_single:
        results["single"] = run_single_agent(args.timeout, args.check_interval)

    if run_multi:
        results["swarm"] = run_swarm(args.timeout, args.check_interval)

    if results:
        print_comparison(results)
    else:
        print("No trials were run.")

    # Write results to JSON
    results_file = os.path.join(BENCHMARK_DIR, "results.json")
    with open(results_file, "w") as f:
        json.dump(results, f, indent=2, default=str)
    print(f"\n  Results saved to: {results_file}")


if __name__ == "__main__":
    main()
