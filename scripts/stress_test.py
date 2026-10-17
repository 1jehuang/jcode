#!/usr/bin/env python3
"""
jcode Stress Test: Spawn 40 sessions via debug socket, measure performance.

Tests:
  1. Session creation throughput
  2. Memory growth per session
  3. FD leak detection
  4. Socket responsiveness under load
  5. Message handling under load (optional)
  6. Session cleanup / resource recovery
"""

import socket
import json
import time
import os
import sys
import subprocess
import signal

NUM_INSTANCES = int(sys.argv[1]) if len(sys.argv) > 1 else 40
MAIN_SOCK = f"/run/user/{os.getuid()}/jcode.sock"
DEBUG_SOCK = f"/run/user/{os.getuid()}/jcode-debug.sock"

class Colors:
    BOLD = "\033[1m"
    GREEN = "\033[32m"
    YELLOW = "\033[33m"
    RED = "\033[31m"
    CYAN = "\033[36m"
    DIM = "\033[2m"
    RESET = "\033[0m"

def debug_cmd(cmd, timeout=10):
    """Send a debug command and return parsed response."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    try:
        sock.connect(DEBUG_SOCK)
        req = {"type": "debug_command", "id": 1, "command": cmd}
        sock.send((json.dumps(req) + "\n").encode())
        
        # Read until we get a complete JSON response
        buf = b""
        while True:
            chunk = sock.recv(65536)
            if not chunk:
                break
            buf += chunk
            try:
                return json.loads(buf.decode())
            except json.JSONDecodeError:
                continue
    except Exception as e:
        return {"error": str(e)}
    finally:
        sock.close()

def get_server_pid():
    """Get the jcode server PID."""
    try:
        result = subprocess.run(
            ["lsof", "-U", "-a", "-c", "jcode"],
            capture_output=True, text=True, timeout=5
        )
        for line in result.stdout.splitlines():
            if "kcode.sock" in line and "LISTEN" in line:
                parts = line.split()
                return int(parts[1])
    except:
        pass
    
    # Fallback: find the oldest jcode process (likely the server)
    try:
        result = subprocess.run(
            ["pgrep", "-o", "jcode"], capture_output=True, text=True, timeout=5
        )
        if result.stdout.strip():
            return int(result.stdout.strip().splitlines()[0])
    except:
        pass
    return None

def proc_stat(pid):
    """Get process stats from /proc."""
    stats = {}
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    stats["rss_kb"] = int(line.split()[1])
                elif line.startswith("VmSize:"):
                    stats["vms_kb"] = int(line.split()[1])
                elif line.startswith("Threads:"):
                    stats["threads"] = int(line.split()[1])
        stats["fds"] = len(os.listdir(f"/proc/{pid}/fd"))
    except:
        pass
    return stats

def fmt_kb(kb):
    if kb > 1024*1024:
        return f"{kb/1024/1024:.1f}GB"
    elif kb > 1024:
        return f"{kb/1024:.1f}MB"
    return f"{kb}KB"

def print_header(text):
    print(f"\n{Colors.BOLD}{Colors.CYAN}{'='*60}")
    print(f" {text}")
    print(f"{'='*60}{Colors.RESET}\n")

def print_section(text):
    print(f"\n{Colors.BOLD}--- {text} ---{Colors.RESET}")

def print_ok(text):
    print(f"  {Colors.GREEN}✅ {text}{Colors.RESET}")

def print_warn(text):
    print(f"  {Colors.YELLOW}⚠️  {text}{Colors.RESET}")

def print_err(text):
    print(f"  {Colors.RED}❌ {text}{Colors.RESET}")

def print_stat(label, value):
    print(f"  {label}: {Colors.BOLD}{value}{Colors.RESET}")

# ============================================================
# MAIN
# ============================================================

print_header(f"jcode Stress Test: {NUM_INSTANCES} sessions")

# --- Pre-flight ---
print_section("Pre-flight checks")

if not os.path.exists(MAIN_SOCK):
    print_err(f"No server socket at {MAIN_SOCK}")
    print("  Start a server with: jcode serve &")
    sys.exit(1)

if not os.path.exists(DEBUG_SOCK):
    print_err(f"No debug socket at {DEBUG_SOCK}")
    print("  Enable with: touch ~/.kcode/debug_control")
    sys.exit(1)

# Test connectivity
t0 = time.monotonic()
state = debug_cmd("state")
t1 = time.monotonic()
if "error" in state:
    print_err(f"Debug socket error: {state['error']}")
    sys.exit(1)
print_ok(f"Debug socket responding ({(t1-t0)*1000:.0f}ms)")

server_pid = get_server_pid()
if server_pid:
    print_ok(f"Server PID: {server_pid}")
else:
    print_warn("Could not determine server PID")

# --- Baseline ---
print_section("Baseline measurements")

baseline = proc_stat(server_pid) if server_pid else {}
baseline_procs = int(subprocess.run(["pgrep", "-c", "jcode"], capture_output=True, text=True).stdout.strip() or "0")

print_stat("Server RSS", fmt_kb(baseline.get("rss_kb", 0)))
print_stat("Server VMS", fmt_kb(baseline.get("vms_kb", 0)))
print_stat("Server FDs", baseline.get("fds", "?"))
print_stat("Server threads", baseline.get("threads", "?"))
print_stat("Total jcode procs", baseline_procs)

# List existing sessions
sessions_resp = debug_cmd("sessions")
try:
    existing_sessions = json.loads(sessions_resp.get("output", "[]"))
    print_stat("Existing sessions", len(existing_sessions))
except:
    existing_sessions = []
    print_stat("Existing sessions", "?")

# --- Phase 1: Rapid session creation ---
print_section(f"Phase 1: Creating {NUM_INSTANCES} sessions")

created_sessions = []
create_times = []
create_errors = []
per_session_stats = []

for i in range(1, NUM_INSTANCES + 1):
    t0 = time.monotonic()
    resp = debug_cmd(f"create_session:/tmp/jcode-stress-{i}", timeout=30)
    t1 = time.monotonic()
    elapsed_ms = (t1 - t0) * 1000
    create_times.append(elapsed_ms)
    
    if resp.get("ok"):
        try:
            session_data = json.loads(resp["output"])
            session_id = session_data.get("session_id", "")
            created_sessions.append(session_id)
        except:
            created_sessions.append(f"unknown_{i}")
            create_errors.append((i, "Failed to parse session ID"))
    else:
        create_errors.append((i, resp.get("error", resp.get("output", "unknown"))))

    # Progress + snapshot every 10
    if i % 10 == 0 or i == NUM_INSTANCES:
        stats = proc_stat(server_pid) if server_pid else {}
        per_session_stats.append((i, stats.copy()))
        rss = fmt_kb(stats.get("rss_kb", 0))
        fds = stats.get("fds", "?")
        threads = stats.get("threads", "?")
        avg_ms = sum(create_times[-10:]) / len(create_times[-10:])
        print(f"  [{i:3d}/{NUM_INSTANCES}] avg_create={avg_ms:.0f}ms rss={rss} fds={fds} threads={threads} sessions_ok={len(created_sessions)}")
    elif i % 5 == 0:
        sys.stdout.write(".")
        sys.stdout.flush()

if create_errors:
    print_warn(f"{len(create_errors)} creation errors:")
    for idx, err in create_errors[:5]:
        print(f"    Instance {idx}: {err}")
    if len(create_errors) > 5:
        print(f"    ... and {len(create_errors) - 5} more")
else:
    print_ok(f"All {NUM_INSTANCES} sessions created successfully")

# --- Phase 2: Socket responsiveness under load ---
print_section("Phase 2: Socket responsiveness with all sessions active")

socket_times = []
for probe in range(1, 11):
    t0 = time.monotonic()
    resp = debug_cmd("state", timeout=15)
    t1 = time.monotonic()
    elapsed_ms = (t1 - t0) * 1000
    socket_times.append(elapsed_ms)

if socket_times:
    print_stat("Debug cmd min", f"{min(socket_times):.0f}ms")
    print_stat("Debug cmd max", f"{max(socket_times):.0f}ms")
    print_stat("Debug cmd avg", f"{sum(socket_times)/len(socket_times):.0f}ms")
    if max(socket_times) > 1000:
        print_warn(f"Socket latency exceeded 1s ({max(socket_times):.0f}ms)")

# List sessions under load
t0 = time.monotonic()
sessions_resp = debug_cmd("sessions", timeout=30)
t1 = time.monotonic()
try:
    all_sessions = json.loads(sessions_resp.get("output", "[]"))
    print_stat("Sessions list time", f"{(t1-t0)*1000:.0f}ms ({len(all_sessions)} sessions)")
except:
    print_stat("Sessions list time", f"{(t1-t0)*1000:.0f}ms (parse error)")

# --- Phase 3: Send a message to a few sessions ---
print_section("Phase 3: Message handling under load (5 sessions)")

message_times = []
for idx, sid in enumerate(created_sessions[:5]):
    t0 = time.monotonic()
    # Use tool execution instead of message (avoids LLM call)
    resp = debug_cmd(f"tool:bash {{\"command\":\"echo stress_test_{idx}\"}}", timeout=30)
    t1 = time.monotonic()
    elapsed_ms = (t1 - t0) * 1000
    message_times.append(elapsed_ms)
    status = "ok" if resp.get("ok") else "err"
    print(f"  Session {idx+1}: tool exec {elapsed_ms:.0f}ms ({status})")

if message_times:
    print_stat("Tool exec avg", f"{sum(message_times)/len(message_times):.0f}ms")

# --- Phase 4: Peak resource measurement ---
print_section("Phase 4: Peak resource usage")

peak = proc_stat(server_pid) if server_pid else {}
peak_procs = int(subprocess.run(["pgrep", "-c", "jcode"], capture_output=True, text=True).stdout.strip() or "0")

print_stat("Server RSS", fmt_kb(peak.get("rss_kb", 0)))
print_stat("Server VMS", fmt_kb(peak.get("vms_kb", 0)))
print_stat("Server FDs", peak.get("fds", "?"))
print_stat("Server threads", peak.get("threads", "?"))
print_stat("Total jcode procs", peak_procs)
print_stat("RSS per session (approx)", fmt_kb(
    max(0, (peak.get("rss_kb", 0) - baseline.get("rss_kb", 0))) // max(1, len(created_sessions))
))

# --- Phase 5: Destroy all sessions ---
print_section(f"Phase 5: Destroying {len(created_sessions)} sessions")

destroy_times = []
destroy_errors = []

for i, sid in enumerate(created_sessions, 1):
    t0 = time.monotonic()
    resp = debug_cmd(f"destroy_session:{sid}", timeout=15)
    t1 = time.monotonic()
    elapsed_ms = (t1 - t0) * 1000
    destroy_times.append(elapsed_ms)
    
    if not resp.get("ok"):
        destroy_errors.append((i, resp.get("error", resp.get("output", "unknown"))))

    if i % 10 == 0 or i == len(created_sessions):
        stats = proc_stat(server_pid) if server_pid else {}
        rss = fmt_kb(stats.get("rss_kb", 0))
        fds = stats.get("fds", "?")
        avg_ms = sum(destroy_times[-10:]) / len(destroy_times[-10:])
        print(f"  [{i:3d}/{len(created_sessions)}] avg_destroy={avg_ms:.0f}ms rss={rss} fds={fds}")

if destroy_errors:
    print_warn(f"{len(destroy_errors)} destroy errors")
    for idx, err in destroy_errors[:3]:
        print(f"    Session {idx}: {err}")
else:
    print_ok(f"All {len(created_sessions)} sessions destroyed")

# --- Phase 6: Resource recovery check ---
print_section("Phase 6: Resource recovery (waiting 10s for cleanup)")

time.sleep(10)

final = proc_stat(server_pid) if server_pid else {}
final_procs = int(subprocess.run(["pgrep", "-c", "jcode"], capture_output=True, text=True).stdout.strip() or "0")

# Final socket test
t0 = time.monotonic()
final_state = debug_cmd("state")
t1 = time.monotonic()
final_socket_ms = (t1 - t0) * 1000

print_stat("Server RSS", f"{fmt_kb(final.get('rss_kb', 0))} (was {fmt_kb(baseline.get('rss_kb', 0))})")
print_stat("Server FDs", f"{final.get('fds', '?')} (was {baseline.get('fds', '?')})")
print_stat("Server threads", f"{final.get('threads', '?')} (was {baseline.get('threads', '?')})")
print_stat("Socket latency", f"{final_socket_ms:.0f}ms")

# Check for leaks
rss_delta = final.get("rss_kb", 0) - baseline.get("rss_kb", 0)
fd_delta = (final.get("fds", 0) or 0) - (baseline.get("fds", 0) or 0)
thread_delta = (final.get("threads", 0) or 0) - (baseline.get("threads", 0) or 0)

# ============================================================
# FINAL REPORT
# ============================================================

print_header("STRESS TEST RESULTS")

print(f"  {Colors.BOLD}Sessions:{Colors.RESET} {NUM_INSTANCES} created, {len(created_sessions)} successful, {len(create_errors)} failed")
print()

print(f"  {Colors.BOLD}Session Creation:{Colors.RESET}")
if create_times:
    print(f"    Min: {min(create_times):.0f}ms")
    print(f"    Max: {max(create_times):.0f}ms")
    print(f"    Avg: {sum(create_times)/len(create_times):.0f}ms")
    print(f"    p95: {sorted(create_times)[int(len(create_times)*0.95)]:.0f}ms")
    print(f"    Total: {sum(create_times):.0f}ms")

print()
print(f"  {Colors.BOLD}Session Destruction:{Colors.RESET}")
if destroy_times:
    print(f"    Min: {min(destroy_times):.0f}ms")
    print(f"    Max: {max(destroy_times):.0f}ms")
    print(f"    Avg: {sum(destroy_times)/len(destroy_times):.0f}ms")

print()
print(f"  {Colors.BOLD}Memory:{Colors.RESET}")
print(f"    Baseline RSS: {fmt_kb(baseline.get('rss_kb', 0))}")
print(f"    Peak RSS:     {fmt_kb(peak.get('rss_kb', 0))}")
print(f"    Final RSS:    {fmt_kb(final.get('rss_kb', 0))}")
print(f"    RSS delta:    {'+' if rss_delta >= 0 else ''}{fmt_kb(abs(rss_delta))}")
print(f"    Per-session:  ~{fmt_kb(max(0, peak.get('rss_kb', 0) - baseline.get('rss_kb', 0)) // max(1, len(created_sessions)))}")

print()
print(f"  {Colors.BOLD}Resource Leaks:{Colors.RESET}")

fd_ok = abs(fd_delta) <= 10
thread_ok = abs(thread_delta) <= 5
rss_ok = rss_delta < (baseline.get("rss_kb", 0) or 100000) * 0.2  # <20% growth

if fd_ok:
    print_ok(f"FDs: {baseline.get('fds','?')} -> {final.get('fds','?')} (delta: {fd_delta})")
else:
    print_warn(f"FD leak: {baseline.get('fds','?')} -> {final.get('fds','?')} (delta: {fd_delta})")

if thread_ok:
    print_ok(f"Threads: {baseline.get('threads','?')} -> {final.get('threads','?')} (delta: {thread_delta})")
else:
    print_warn(f"Thread leak: {baseline.get('threads','?')} -> {final.get('threads','?')} (delta: {thread_delta})")

if rss_ok:
    print_ok(f"Memory: {fmt_kb(baseline.get('rss_kb',0))} -> {fmt_kb(final.get('rss_kb',0))} (delta: {fmt_kb(abs(rss_delta))})")
else:
    print_warn(f"Memory not fully recovered: {fmt_kb(baseline.get('rss_kb',0))} -> {fmt_kb(final.get('rss_kb',0))} (delta: {fmt_kb(abs(rss_delta))})")

print()
print(f"  {Colors.BOLD}Socket Health:{Colors.RESET}")
if final_socket_ms < 100:
    print_ok(f"Responsive after stress: {final_socket_ms:.0f}ms")
elif final_socket_ms < 1000:
    print_warn(f"Slow after stress: {final_socket_ms:.0f}ms")
else:
    print_err(f"Very slow after stress: {final_socket_ms:.0f}ms")

# Memory growth timeline
print()
print(f"  {Colors.BOLD}Memory Growth Timeline:{Colors.RESET}")
print(f"    {'Sessions':>10s}  {'RSS':>10s}  {'FDs':>6s}  {'Threads':>8s}")
print(f"    {'─'*10}  {'─'*10}  {'─'*6}  {'─'*8}")
print(f"    {'baseline':>10s}  {fmt_kb(baseline.get('rss_kb',0)):>10s}  {str(baseline.get('fds','?')):>6s}  {str(baseline.get('threads','?')):>8s}")
for count, stats in per_session_stats:
    print(f"    {count:>10d}  {fmt_kb(stats.get('rss_kb',0)):>10s}  {str(stats.get('fds','?')):>6s}  {str(stats.get('threads','?')):>8s}")
print(f"    {'final':>10s}  {fmt_kb(final.get('rss_kb',0)):>10s}  {str(final.get('fds','?')):>6s}  {str(final.get('threads','?')):>8s}")

print()
print(f"{'='*60}")
