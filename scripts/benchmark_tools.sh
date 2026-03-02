#!/bin/bash
# Tool call benchmarking script
# Measures execution time for each tool with representative inputs
# Run from the jcode repo root

set -euo pipefail

ITERATIONS=${1:-5}
RESULTS_FILE="/tmp/jcode_tool_benchmark_$(date +%Y%m%d_%H%M%S).csv"

echo "=== jcode Tool Call Benchmark ==="
echo "Iterations per tool: $ITERATIONS"
echo "Results file: $RESULTS_FILE"
echo ""

# CSV header
echo "tool,iteration,time_ms,input_size_bytes,output_size_bytes" > "$RESULTS_FILE"

# Helper: benchmark a tool via the debug socket
benchmark_tool() {
    local tool_name="$1"
    local tool_input="$2"
    local label="${3:-$tool_name}"
    
    local input_size=${#tool_input}
    local total_ms=0
    local min_ms=999999
    local max_ms=0
    local times=()
    
    for i in $(seq 1 "$ITERATIONS"); do
        local start_ns=$(date +%s%N)
        
        # Execute via debug socket
        local output
        output=$(echo "{\"type\":\"debug_command\",\"id\":1,\"command\":\"tool:$tool_name $tool_input\",\"session_id\":\"$SESSION_ID\"}" | \
            socat - UNIX-CONNECT:"$DEBUG_SOCK" 2>/dev/null || echo '{"error":"timeout"}')
        
        local end_ns=$(date +%s%N)
        local elapsed_ms=$(( (end_ns - start_ns) / 1000000 ))
        
        local output_size=${#output}
        echo "$label,$i,$elapsed_ms,$input_size,$output_size" >> "$RESULTS_FILE"
        
        times+=("$elapsed_ms")
        total_ms=$((total_ms + elapsed_ms))
        
        if [ "$elapsed_ms" -lt "$min_ms" ]; then min_ms=$elapsed_ms; fi
        if [ "$elapsed_ms" -gt "$max_ms" ]; then max_ms=$elapsed_ms; fi
    done
    
    local avg_ms=$((total_ms / ITERATIONS))
    
    # Compute p50
    IFS=$'\n' sorted_times=($(sort -n <<<"${times[*]}")); unset IFS
    local p50_idx=$(( ITERATIONS / 2 ))
    local p50_ms=${sorted_times[$p50_idx]}
    
    printf "  %-30s  avg=%4dms  p50=%4dms  min=%4dms  max=%4dms\n" \
        "$label" "$avg_ms" "$p50_ms" "$min_ms" "$max_ms"
}

# Find debug socket
DEBUG_SOCK="${JCODE_DEBUG_SOCK:-/run/user/$(id -u)/jcode-debug.sock}"

if [ ! -S "$DEBUG_SOCK" ]; then
    echo "ERROR: Debug socket not found at $DEBUG_SOCK"
    echo "Make sure jcode is running with debug control enabled."
    exit 1
fi

# Get session ID
echo "Getting session ID..."
SESSION_RESP=$(echo '{"type":"debug_command","id":1,"command":"state"}' | \
    socat -t5 - UNIX-CONNECT:"$DEBUG_SOCK" 2>/dev/null || echo '{}')
SESSION_ID=$(echo "$SESSION_RESP" | python3 -c "
import sys, json
try:
    d = json.loads(sys.stdin.read())
    out = d.get('output', '{}')
    if isinstance(out, str):
        d2 = json.loads(out)
    else:
        d2 = out
    print(d2.get('session_id', ''))
except:
    print('')
" 2>/dev/null)

if [ -z "$SESSION_ID" ]; then
    echo "ERROR: Could not get session ID from debug socket"
    echo "Response: $SESSION_RESP"
    exit 1
fi

echo "Session: $SESSION_ID"
echo ""

# Create temp files for testing
TMPDIR=$(mktemp -d)
echo "hello world" > "$TMPDIR/test.txt"
echo -e "line 1\nline 2\nline 3\nfoo bar\nbaz qux" > "$TMPDIR/multiline.txt"
mkdir -p "$TMPDIR/subdir"
echo "nested" > "$TMPDIR/subdir/nested.txt"

# Large file for read benchmarks
python3 -c "
for i in range(1000):
    print(f'Line {i}: This is a test line with some content for benchmarking purposes. The quick brown fox jumps over the lazy dog.')
" > "$TMPDIR/large.txt"

echo "=== File System Tools ==="

benchmark_tool "read" "{\"file_path\":\"$TMPDIR/test.txt\"}" "read (tiny file)"
benchmark_tool "read" "{\"file_path\":\"$TMPDIR/large.txt\"}" "read (1000 lines)"
benchmark_tool "read" "{\"file_path\":\"$TMPDIR/large.txt\",\"offset\":500,\"limit\":10}" "read (10 lines @ offset)"
benchmark_tool "read" "{\"file_path\":\"src/main.rs\"}" "read (main.rs)"

echo ""
echo "=== Write/Edit Tools ==="

benchmark_tool "write" "{\"file_path\":\"$TMPDIR/write_test.txt\",\"content\":\"hello world\"}" "write (small)"
benchmark_tool "write" "{\"file_path\":\"$TMPDIR/write_test.txt\",\"content\":\"$(python3 -c "print('x' * 10000)")\"}" "write (10KB)"

# Setup file for edit
echo "The quick brown fox jumps over the lazy dog" > "$TMPDIR/edit_test.txt"
benchmark_tool "edit" "{\"file_path\":\"$TMPDIR/edit_test.txt\",\"old_string\":\"quick brown\",\"new_string\":\"slow red\"}" "edit (simple replace)"
# Reset for next iteration
for i in $(seq 1 "$ITERATIONS"); do
    echo "The quick brown fox jumps over the lazy dog" > "$TMPDIR/edit_test.txt"
done

echo ""
echo "=== Search/Navigation Tools ==="

benchmark_tool "grep" "{\"pattern\":\"fn main\",\"path\":\"src\",\"include\":\"*.rs\"}" "grep (fn main in src/)"
benchmark_tool "grep" "{\"pattern\":\"async fn\",\"path\":\"src/tool\",\"include\":\"*.rs\"}" "grep (async fn in tools)"
benchmark_tool "grep" "{\"pattern\":\"tokio::spawn\",\"path\":\"src\"}" "grep (tokio::spawn)"

benchmark_tool "glob" "{\"pattern\":\"**/*.rs\"}" "glob (**/*.rs)"
benchmark_tool "glob" "{\"pattern\":\"**/*.rs\",\"path\":\"src/tool\"}" "glob (tool/*.rs)"

benchmark_tool "ls" "{}" "ls (repo root)"
benchmark_tool "ls" "{\"path\":\"src\"}" "ls (src/)"
benchmark_tool "ls" "{\"path\":\"src/tool\"}" "ls (src/tool/)"

echo ""
echo "=== Shell Tools ==="

benchmark_tool "bash" "{\"command\":\"echo hello\"}" "bash (echo)"
benchmark_tool "bash" "{\"command\":\"true\"}" "bash (true)"
benchmark_tool "bash" "{\"command\":\"ls -la src/tool/\"}" "bash (ls -la)"
benchmark_tool "bash" "{\"command\":\"wc -l src/main.rs\"}" "bash (wc -l)"
benchmark_tool "bash" "{\"command\":\"cat /dev/null\"}" "bash (cat /dev/null)"
benchmark_tool "bash" "{\"command\":\"git log --oneline -5\"}" "bash (git log -5)"
benchmark_tool "bash" "{\"command\":\"cargo --version\"}" "bash (cargo --version)"

echo ""
echo "=== Memory/Search Tools ==="

benchmark_tool "todoread" "{}" "todoread"
benchmark_tool "todowrite" "{\"todos\":[{\"id\":\"bench1\",\"content\":\"benchmark test\",\"status\":\"pending\",\"priority\":\"low\"}]}" "todowrite"
benchmark_tool "conversation_search" "{\"stats\":true}" "conversation_search (stats)"
benchmark_tool "memory" "{\"action\":\"recall\",\"query\":\"benchmark test\",\"limit\":3}" "memory (recall)"
benchmark_tool "memory" "{\"action\":\"list\",\"limit\":5}" "memory (list)"

echo ""
echo "=== Tool Dispatch Overhead ==="

benchmark_tool "invalid" "{\"tool\":\"test\",\"error\":\"benchmark\"}" "invalid (no-op)"

echo ""
echo "=== Results Summary ==="
echo ""

# Parse and summarize
python3 << 'PYEOF'
import csv
from collections import defaultdict

results = defaultdict(list)
with open("RESULTS_FILE_PLACEHOLDER") as f:
    reader = csv.DictReader(f)
    for row in reader:
        results[row['tool']].append(int(row['time_ms']))

# Sort by average time (descending)
summary = []
for tool, times in results.items():
    avg = sum(times) / len(times)
    p50 = sorted(times)[len(times) // 2]
    summary.append((tool, avg, p50, min(times), max(times)))

summary.sort(key=lambda x: x[1], reverse=True)

print(f"{'Tool':<35} {'Avg':>7} {'P50':>7} {'Min':>7} {'Max':>7}")
print("-" * 70)

for tool, avg, p50, mn, mx in summary:
    bar = "█" * max(1, int(avg / 10))
    print(f"{tool:<35} {avg:6.0f}ms {p50:5d}ms {mn:5d}ms {mx:5d}ms  {bar}")

total_avg = sum(s[1] for s in summary)
print(f"\n{'Total (all tools avg sum)':<35} {total_avg:6.0f}ms")
print(f"\nSlowest tool: {summary[0][0]} ({summary[0][1]:.0f}ms avg)")
print(f"Fastest tool: {summary[-1][0]} ({summary[-1][1]:.0f}ms avg)")
PYEOF

# Cleanup
rm -rf "$TMPDIR"

echo ""
echo "Full CSV results: $RESULTS_FILE"
