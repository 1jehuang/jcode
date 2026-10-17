#!/usr/bin/env bash
# memory_regression_gate.sh - Automated pass/fail gate over client memory.
#
# Wraps scripts/memory_probe.sh with thresholds so memory regressions fail
# loudly (nightly job, pre-release check) instead of being rediscovered when
# the machine starts swapping.
#
# Baseline (2026-07-04, selfdev build, 8 MB / 322-message hog session), after
# the retention + live-heap work (mmap-threshold pin, idle trim, syntect
# regex-onig backend, inline-image payload release):
#
#   idle rss_anon:              ~38 MB   (was ~69 MB)
#   idle live heap (allocated): ~30 MB   (was ~60 MB)
#
# Thresholds sit roughly halfway between the fixed and regressed values, so
# normal variance passes but a real regression toward old behavior fails.
#
# Usage:
#   scripts/memory_regression_gate.sh [--session <id>] [--idle-secs <n>]
#
# Env overrides:
#   KCODE_MEMGATE_SESSION               probe session id
#   KCODE_MEMGATE_MAX_RSS_ANON_KB       idle rss_anon bound (default 56320 = 55 MiB)
#   KCODE_MEMGATE_MAX_LIVE_HEAP_BYTES   idle allocated_bytes bound (default 47185920 = 45 MiB)
#
# Exit codes: 0 pass, 1 threshold exceeded, 2 could not measure, 3 skipped
# (probe session missing on this machine).
#
# Emits one machine-parseable line: `KCODE_MEMGATE_RESULT {json}`.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SESSION_ID="${KCODE_MEMGATE_SESSION:-session_hog_1783086065415_4ad4ae66cd43dd5b}"
IDLE_SECS=60
MAX_RSS_ANON_KB="${KCODE_MEMGATE_MAX_RSS_ANON_KB:-56320}"
MAX_LIVE_HEAP_BYTES="${KCODE_MEMGATE_MAX_LIVE_HEAP_BYTES:-47185920}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --session) SESSION_ID="$2"; shift 2 ;;
        --idle-secs) IDLE_SECS="$2"; shift 2 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

command -v jq >/dev/null || { echo "jq required" >&2; exit 2; }

# Skip (not fail) when the pinned probe session does not exist on this
# machine: thresholds are only meaningful against a fixed workload.
if ! ls "$HOME/.kcode/sessions/${SESSION_ID}"* >/dev/null 2>&1; then
    echo "KCODE_MEMGATE_RESULT {\"status\":\"skipped\",\"reason\":\"probe session ${SESSION_ID} not found\"}"
    exit 3
fi

echo "[memgate] probing session ${SESSION_ID} (idle ${IDLE_SECS}s)..." >&2
PROBE_OUT="$("$SCRIPT_DIR/memory_probe.sh" --session "$SESSION_ID" --idle-secs "$IDLE_SECS" --skip-trim)" || {
    echo "KCODE_MEMGATE_RESULT {\"status\":\"error\",\"reason\":\"memory_probe failed\"}"
    exit 2
}

IDLE_LINE="$(printf '%s\n' "$PROBE_OUT" | jq -c 'select(.phase=="idle")' | head -1)"
[[ -n "$IDLE_LINE" ]] || {
    echo "KCODE_MEMGATE_RESULT {\"status\":\"error\",\"reason\":\"no idle phase in probe output\"}"
    exit 2
}

RSS_ANON_KB="$(jq -r '.proc.rss_anon_kb // 0' <<<"$IDLE_LINE")"
LIVE_HEAP_BYTES="$(jq -r '.client.allocated_bytes // 0' <<<"$IDLE_LINE")"

# Informational server-side numbers (never gate: server load varies).
SERVER_INFO="$(jq -cn \
    --argjson rss "$(awk '/VmRSS/{print $2*1024}' "/proc/$(pgrep -f 'shared-server/jcode serve' | head -1)/status" 2>/dev/null || echo 0)" \
    '{server_rss_bytes: $rss}')"

FAILURES=()
if (( RSS_ANON_KB > MAX_RSS_ANON_KB )); then
    FAILURES+=("idle rss_anon ${RSS_ANON_KB}kB > ${MAX_RSS_ANON_KB}kB")
fi
if (( LIVE_HEAP_BYTES > MAX_LIVE_HEAP_BYTES )); then
    FAILURES+=("idle live heap ${LIVE_HEAP_BYTES}B > ${MAX_LIVE_HEAP_BYTES}B")
fi

STATUS="pass"
(( ${#FAILURES[@]} > 0 )) && STATUS="fail"

jq -cn \
    --arg status "$STATUS" \
    --arg session "$SESSION_ID" \
    --argjson rss_anon_kb "$RSS_ANON_KB" \
    --argjson live_heap_bytes "$LIVE_HEAP_BYTES" \
    --argjson max_rss_anon_kb "$MAX_RSS_ANON_KB" \
    --argjson max_live_heap_bytes "$MAX_LIVE_HEAP_BYTES" \
    --argjson server "$SERVER_INFO" \
    --args '{status:$status, session:$session, idle_rss_anon_kb:$rss_anon_kb,
             idle_live_heap_bytes:$live_heap_bytes,
             thresholds:{rss_anon_kb:$max_rss_anon_kb, live_heap_bytes:$max_live_heap_bytes},
             server:$server, failures:$ARGS.positional}' -- "${FAILURES[@]+"${FAILURES[@]}"}" \
    | sed 's/^/KCODE_MEMGATE_RESULT /'

if [[ "$STATUS" == "fail" ]]; then
    printf '[memgate] FAIL: %s\n' "${FAILURES[@]}" >&2
    exit 1
fi
echo "[memgate] PASS: idle rss_anon ${RSS_ANON_KB}kB (<= ${MAX_RSS_ANON_KB}), live heap ${LIVE_HEAP_BYTES}B (<= ${MAX_LIVE_HEAP_BYTES})" >&2
