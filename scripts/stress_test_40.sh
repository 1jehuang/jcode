#!/usr/bin/env bash
#
# Stress test: spawn 40 jcode TUI client instances rapidly
# Measures startup time, memory usage, CPU, fd count, socket health
#
set -euo pipefail

NUM_INSTANCES=${1:-40}
KCODE_BIN="${KCODE_BIN:-$(which jcode)}"
LOG_DIR="/tmp/jcode-stress-test-$(date +%s)"
mkdir -p "$LOG_DIR"

MAIN_SOCK="/run/user/$(id -u)/jcode.sock"
DEBUG_SOCK="/run/user/$(id -u)/jcode-debug.sock"

echo "========================================="
echo " jcode Stress Test: $NUM_INSTANCES instances"
echo "========================================="
echo "Binary: $KCODE_BIN"
echo "Log dir: $LOG_DIR"
echo "Main socket: $MAIN_SOCK"
echo ""

# --- Helper functions ---

get_server_pid() {
    # Server listens on the main socket
    lsof -U 2>/dev/null | grep "$(basename $MAIN_SOCK)" | awk '{print $2}' | sort -u | head -1
}

snapshot() {
    local label="$1"
    local ts=$(date +%s%N)

    echo "--- Snapshot: $label ---" >> "$LOG_DIR/snapshots.log"
    echo "timestamp_ns: $ts" >> "$LOG_DIR/snapshots.log"

    # Memory
    free -m >> "$LOG_DIR/snapshots.log" 2>/dev/null
    echo "" >> "$LOG_DIR/snapshots.log"

    # jcode process count and total RSS
    local jcode_procs=$(pgrep -c jcode 2>/dev/null || echo 0)
    local total_rss=0
    local total_vms=0
    for pid in $(pgrep jcode 2>/dev/null); do
        local rss=$(awk '/^VmRSS:/{print $2}' /proc/$pid/status 2>/dev/null || echo 0)
        local vms=$(awk '/^VmSize:/{print $2}' /proc/$pid/status 2>/dev/null || echo 0)
        total_rss=$((total_rss + rss))
        total_vms=$((total_vms + vms))
    done
    echo "jcode_processes: $jcode_procs" >> "$LOG_DIR/snapshots.log"
    echo "total_rss_kb: $total_rss" >> "$LOG_DIR/snapshots.log"
    echo "total_vms_kb: $total_vms" >> "$LOG_DIR/snapshots.log"

    # Open file descriptors for server
    local server_pid=$(get_server_pid)
    if [ -n "$server_pid" ]; then
        local fd_count=$(ls /proc/$server_pid/fd 2>/dev/null | wc -l)
        local thread_count=$(ls /proc/$server_pid/task 2>/dev/null | wc -l)
        echo "server_pid: $server_pid" >> "$LOG_DIR/snapshots.log"
        echo "server_fd_count: $fd_count" >> "$LOG_DIR/snapshots.log"
        echo "server_threads: $thread_count" >> "$LOG_DIR/snapshots.log"
        # Server RSS specifically
        local server_rss=$(awk '/^VmRSS:/{print $2}' /proc/$server_pid/status 2>/dev/null || echo 0)
        echo "server_rss_kb: $server_rss" >> "$LOG_DIR/snapshots.log"
    fi

    # CPU load
    cat /proc/loadavg >> "$LOG_DIR/snapshots.log"
    echo "" >> "$LOG_DIR/snapshots.log"
    echo "===" >> "$LOG_DIR/snapshots.log"

    # Print summary line to stdout
    echo "[$label] procs=$jcode_procs rss=${total_rss}KB($(( total_rss / 1024 ))MB) server_rss=$(awk '/^VmRSS:/{print $2}' /proc/${server_pid:-0}/status 2>/dev/null || echo '?')KB fds=$(ls /proc/${server_pid:-0}/fd 2>/dev/null | wc -l) threads=$(ls /proc/${server_pid:-0}/task 2>/dev/null | wc -l)"
}

check_socket_health() {
    local label="$1"
    # Try a quick connect-and-disconnect on the main socket
    local start_ns=$(date +%s%N)
    if python3 -c "
import socket, json, sys, time
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    sock.settimeout(5)
    sock.connect('$MAIN_SOCK')
    # just connect and close - test socket responsiveness
    sock.close()
    sys.exit(0)
except Exception as e:
    print(f'Socket error: {e}', file=sys.stderr)
    sys.exit(1)
" 2>>"$LOG_DIR/socket_errors.log"; then
        local end_ns=$(date +%s%N)
        local dur_ms=$(( (end_ns - start_ns) / 1000000 ))
        echo "[$label] Socket connect: ${dur_ms}ms" | tee -a "$LOG_DIR/socket_latency.log"
    else
        echo "[$label] Socket connect: FAILED" | tee -a "$LOG_DIR/socket_latency.log"
    fi
}

debug_cmd() {
    local cmd="$1"
    local timeout="${2:-5}"
    python3 -c "
import socket, json, sys
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    sock.settimeout($timeout)
    sock.connect('$DEBUG_SOCK')
    req = {'type': 'debug_command', 'id': 1, 'command': '$cmd'}
    sock.send((json.dumps(req) + '\n').encode())
    data = sock.recv(65536).decode()
    print(data)
    sock.close()
except Exception as e:
    print(json.dumps({'error': str(e)}))
" 2>/dev/null
}

# --- Pre-flight ---

echo "=== Pre-flight checks ==="

# Check if server is running
if ! [ -S "$MAIN_SOCK" ]; then
    echo "ERROR: No jcode server running at $MAIN_SOCK"
    echo "Start one with: jcode serve &"
    exit 1
fi

# Test socket
check_socket_health "pre-flight"

# Baseline snapshot
snapshot "baseline"
echo ""

# --- Record system baseline ---
BASELINE_RSS=$(pgrep jcode 2>/dev/null | while read pid; do awk '/^VmRSS:/{print $2}' /proc/$pid/status 2>/dev/null; done | paste -sd+ | bc 2>/dev/null || echo 0)
BASELINE_PROCS=$(pgrep -c jcode 2>/dev/null || echo 0)
BASELINE_SERVER_PID=$(get_server_pid)
BASELINE_FDS=$(ls /proc/${BASELINE_SERVER_PID:-0}/fd 2>/dev/null | wc -l)

echo "=== Baseline ==="
echo "  Processes: $BASELINE_PROCS"
echo "  Total RSS: ${BASELINE_RSS}KB ($(( BASELINE_RSS / 1024 ))MB)"
echo "  Server FDs: $BASELINE_FDS"
echo ""

# --- Start background monitoring ---
echo "=== Starting background monitor ==="
(
    while true; do
        ts=$(date +%s)
        jcode_procs=$(pgrep -c jcode 2>/dev/null || echo 0)
        total_rss=0
        for pid in $(pgrep jcode 2>/dev/null); do
            rss=$(awk '/^VmRSS:/{print $2}' /proc/$pid/status 2>/dev/null || echo 0)
            total_rss=$((total_rss + rss))
        done
        server_pid=$(get_server_pid)
        server_rss=$(awk '/^VmRSS:/{print $2}' /proc/${server_pid:-0}/status 2>/dev/null || echo 0)
        server_fds=$(ls /proc/${server_pid:-0}/fd 2>/dev/null | wc -l)
        server_threads=$(ls /proc/${server_pid:-0}/task 2>/dev/null | wc -l)
        cpu_load=$(awk '{print $1}' /proc/loadavg)
        echo "$ts,$jcode_procs,$total_rss,$server_rss,$server_fds,$server_threads,$cpu_load"
        sleep 1
    done
) > "$LOG_DIR/timeseries.csv" &
MONITOR_PID=$!
echo "Monitor PID: $MONITOR_PID"
echo ""

# --- Spawn instances ---
echo "=== Spawning $NUM_INSTANCES jcode instances ==="
PIDS=()
SPAWN_TIMES=()

# Use script to give each instance a pty (jcode requires tty)
for i in $(seq 1 $NUM_INSTANCES); do
    local_start=$(date +%s%N)

    # Each instance gets its own pseudo-terminal via script(1)
    # We connect to the existing server, which creates sessions
    script -q -c "$KCODE_BIN --no-update --no-selfdev" /dev/null \
        > "$LOG_DIR/instance_${i}_stdout.log" \
        2> "$LOG_DIR/instance_${i}_stderr.log" &
    pid=$!
    PIDS+=($pid)

    local_end=$(date +%s%N)
    spawn_ms=$(( (local_end - local_start) / 1000000 ))
    SPAWN_TIMES+=($spawn_ms)

    # Log it
    echo "  [$i/$NUM_INSTANCES] PID=$pid spawn=${spawn_ms}ms" | tee -a "$LOG_DIR/spawn.log"

    # Snapshot every 10 instances
    if (( i % 10 == 0 )); then
        sleep 1  # Let things settle
        snapshot "after_${i}_spawns"
        check_socket_health "after_${i}_spawns"
    fi

    # Small delay to avoid overwhelming everything at once
    sleep 0.2
done

echo ""
echo "=== All $NUM_INSTANCES instances spawned ==="
echo ""

# Let them run for a bit
echo "=== Letting instances stabilize for 10 seconds ==="
sleep 10
snapshot "post_spawn_settled"
check_socket_health "post_spawn_settled"
echo ""

# --- Debug socket probe under load ---
echo "=== Debug socket responsiveness under load ==="
for probe in 1 2 3; do
    start_ns=$(date +%s%N)
    result=$(debug_cmd "state" 10)
    end_ns=$(date +%s%N)
    dur_ms=$(( (end_ns - start_ns) / 1000000 ))
    echo "  Probe $probe: ${dur_ms}ms" | tee -a "$LOG_DIR/debug_probe.log"
    sleep 0.5
done
echo ""

# --- Session listing under load ---
echo "=== Session list under load ==="
start_ns=$(date +%s%N)
sessions_result=$(debug_cmd "sessions" 15)
end_ns=$(date +%s%N)
dur_ms=$(( (end_ns - start_ns) / 1000000 ))
session_count=$(echo "$sessions_result" | python3 -c "
import json, sys
try:
    data = json.loads(sys.stdin.read())
    output = data.get('output', '')
    sessions = json.loads(output) if output else []
    print(len(sessions))
except:
    print('?')
" 2>/dev/null)
echo "  Sessions: $session_count, query time: ${dur_ms}ms"
echo ""

# --- Kill all spawned instances ---
echo "=== Killing all spawned instances ==="
for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
done
# Wait for them to die
sleep 3
# Force kill stragglers
for pid in "${PIDS[@]}"; do
    kill -9 "$pid" 2>/dev/null || true
done
sleep 2

snapshot "post_kill"
echo ""

# --- Post-kill: check for leaked resources ---
echo "=== Post-kill resource check ==="
POST_PROCS=$(pgrep -c jcode 2>/dev/null || echo 0)
POST_RSS=0
for pid in $(pgrep jcode 2>/dev/null); do
    rss=$(awk '/^VmRSS:/{print $2}' /proc/$pid/status 2>/dev/null || echo 0)
    POST_RSS=$((POST_RSS + rss))
done
POST_SERVER_PID=$(get_server_pid)
POST_FDS=$(ls /proc/${POST_SERVER_PID:-0}/fd 2>/dev/null | wc -l)
POST_SERVER_RSS=$(awk '/^VmRSS:/{print $2}' /proc/${POST_SERVER_PID:-0}/status 2>/dev/null || echo 0)

echo "  Processes: $BASELINE_PROCS -> $POST_PROCS (delta: $((POST_PROCS - BASELINE_PROCS)))"
echo "  Total RSS: ${BASELINE_RSS}KB -> ${POST_RSS}KB (delta: $((POST_RSS - BASELINE_RSS))KB)"
echo "  Server FDs: $BASELINE_FDS -> $POST_FDS (delta: $((POST_FDS - BASELINE_FDS)))"
echo "  Server RSS: ${POST_SERVER_RSS}KB"
echo ""

# Check socket health after cleanup
echo "=== Post-cleanup socket health ==="
check_socket_health "post_cleanup_1"
sleep 1
check_socket_health "post_cleanup_2"
echo ""

# --- Wait a bit more and check for memory leak ---
echo "=== Waiting 15s for GC/cleanup ==="
sleep 15
snapshot "post_gc"
FINAL_SERVER_PID=$(get_server_pid)
FINAL_FDS=$(ls /proc/${FINAL_SERVER_PID:-0}/fd 2>/dev/null | wc -l)
FINAL_SERVER_RSS=$(awk '/^VmRSS:/{print $2}' /proc/${FINAL_SERVER_PID:-0}/status 2>/dev/null || echo 0)
check_socket_health "final"
echo ""

# --- Stop background monitor ---
kill $MONITOR_PID 2>/dev/null || true

# --- Summary report ---
echo "========================================="
echo " STRESS TEST SUMMARY"
echo "========================================="
echo ""
echo "Configuration:"
echo "  Instances spawned: $NUM_INSTANCES"
echo "  Binary: $KCODE_BIN"
echo ""

# Spawn time stats
if [ ${#SPAWN_TIMES[@]} -gt 0 ]; then
    total=0
    min=${SPAWN_TIMES[0]}
    max=${SPAWN_TIMES[0]}
    for t in "${SPAWN_TIMES[@]}"; do
        total=$((total + t))
        if (( t < min )); then min=$t; fi
        if (( t > max )); then max=$t; fi
    done
    avg=$((total / ${#SPAWN_TIMES[@]}))
    echo "Spawn Times (fork+exec overhead):"
    echo "  Min: ${min}ms"
    echo "  Max: ${max}ms"
    echo "  Avg: ${avg}ms"
    echo "  Total: ${total}ms"
    echo ""
fi

echo "Memory Impact:"
echo "  Baseline total RSS: ${BASELINE_RSS}KB ($(( BASELINE_RSS / 1024 ))MB)"
echo "  Peak server RSS: (see timeseries)"
echo "  Final server RSS: ${FINAL_SERVER_RSS}KB ($(( FINAL_SERVER_RSS / 1024 ))MB)"
echo "  Server RSS delta from baseline: $((FINAL_SERVER_RSS - $(awk '/^VmRSS:/{print $2}' /proc/${BASELINE_SERVER_PID:-0}/status 2>/dev/null || echo $FINAL_SERVER_RSS)))KB"
echo ""

echo "File Descriptors (leak check):"
echo "  Baseline: $BASELINE_FDS"
echo "  After kill: $POST_FDS (delta: $((POST_FDS - BASELINE_FDS)))"
echo "  After GC: $FINAL_FDS (delta: $((FINAL_FDS - BASELINE_FDS)))"
if (( FINAL_FDS > BASELINE_FDS + 5 )); then
    echo "  ⚠️  POSSIBLE FD LEAK: $((FINAL_FDS - BASELINE_FDS)) fds not cleaned up"
else
    echo "  ✅ FDs cleaned up properly"
fi
echo ""

echo "Socket Latency:"
if [ -f "$LOG_DIR/socket_latency.log" ]; then
    cat "$LOG_DIR/socket_latency.log"
fi
echo ""

echo "Debug Socket Latency:"
if [ -f "$LOG_DIR/debug_probe.log" ]; then
    cat "$LOG_DIR/debug_probe.log"
fi
echo ""

# Check for errors
echo "Errors:"
error_count=0
for f in "$LOG_DIR"/instance_*_stderr.log; do
    if [ -s "$f" ]; then
        instance=$(basename "$f" | sed 's/instance_\([0-9]*\)_.*/\1/')
        errors=$(grep -i "error\|panic\|crash\|failed\|refused" "$f" 2>/dev/null | head -3)
        if [ -n "$errors" ]; then
            echo "  Instance $instance:"
            echo "$errors" | sed 's/^/    /'
            error_count=$((error_count + 1))
        fi
    fi
done
if (( error_count == 0 )); then
    echo "  ✅ No errors detected"
else
    echo ""
    echo "  ⚠️  $error_count instances had errors"
fi
echo ""

# Generate timeseries summary
echo "Timeseries data: $LOG_DIR/timeseries.csv"
echo "  Format: timestamp,procs,total_rss_kb,server_rss_kb,server_fds,server_threads,cpu_load"
if [ -f "$LOG_DIR/timeseries.csv" ]; then
    echo "  Rows: $(wc -l < "$LOG_DIR/timeseries.csv")"
    echo ""
    echo "  Peak values from timeseries:"
    awk -F, '{
        if ($2+0 > max_procs) max_procs=$2+0;
        if ($3+0 > max_rss) max_rss=$3+0;
        if ($4+0 > max_srv_rss) max_srv_rss=$4+0;
        if ($5+0 > max_fds) max_fds=$5+0;
        if ($6+0 > max_threads) max_threads=$6+0;
    } END {
        printf "    Max processes: %d\n", max_procs;
        printf "    Max total RSS: %d KB (%d MB)\n", max_rss, max_rss/1024;
        printf "    Max server RSS: %d KB (%d MB)\n", max_srv_rss, max_srv_rss/1024;
        printf "    Max server FDs: %d\n", max_fds;
        printf "    Max server threads: %d\n", max_threads;
    }' "$LOG_DIR/timeseries.csv"
fi
echo ""

echo "Full logs: $LOG_DIR/"
echo "========================================="
