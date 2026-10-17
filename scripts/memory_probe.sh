#!/usr/bin/env bash
# memory_probe.sh - Reproducible memory probe harness for jcode TUI clients.
#
# Spawns a headless tester TUI via the debug socket (tester:spawn) that resumes
# a LARGE existing session, then records RSS/PSS/rss_anon at fixed phases:
#
#   fresh               - right after spawn, before the event loop settles
#   post_connect        - first successful client debug "state" response
#   post_history_loaded - display messages applied and stable
#   idle                - after --idle-secs (default 30) of idle time
#   post_trim           - after a forced allocator trim (gdb malloc_trim(0))
#
# Each phase emits exactly ONE compact, key-sorted JSON line on stdout so runs
# are diffable (all human logging goes to stderr). Memory is sampled from
# /proc/<pid>/status + /proc/<pid>/smaps_rollup, plus the client's own
# aggregate memory profile (the same handler that serves `client:memory`),
# fetched through the tester's file-based debug channel.
#
# Usage:
#   scripts/memory_probe.sh [options]
#
# Options:
#   --session <id>    Session to resume (default: session_hog_1783086065415_4ad4ae66cd43dd5b)
#   --idle-secs <n>   Idle wait before the idle phase (default: 30)
#   --binary <path>   Client binary (default: ~/.kcode/builds/current/jcode)
#   --jcode <path>    jcode CLI used for `jcode debug ...` (default: ~/.local/bin/jcode)
#   --cwd <path>      Working directory for the tester (default: $HOME)
#   --skip-trim       Skip the forced-trim phase
#   --keep            Do not stop the tester on exit (for manual inspection)
#   -h | --help       Show this help
#
# Requirements: a running jcode server with the debug socket enabled, jq,
# and (for the trim phase) gdb. Trim prefers `sudo -n gdb` because
# kernel.yama.ptrace_scope=1 blocks same-uid attach to non-children.

set -euo pipefail

SESSION_ID="session_hog_1783086065415_4ad4ae66cd43dd5b"
IDLE_SECS=30
KCODE_BIN="${KCODE_BIN:-$HOME/.local/bin/jcode}"
CLIENT_BIN="$HOME/.kcode/builds/current/jcode"
TESTER_CWD="$HOME"
SKIP_TRIM=0
KEEP_TESTER=0

usage() { sed -n '2,35p' "$0" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --session)   SESSION_ID="$2"; shift 2 ;;
        --idle-secs) IDLE_SECS="$2"; shift 2 ;;
        --binary)    CLIENT_BIN="$2"; shift 2 ;;
        --jcode)     KCODE_BIN="$2"; shift 2 ;;
        --cwd)       TESTER_CWD="$2"; shift 2 ;;
        --skip-trim) SKIP_TRIM=1; shift ;;
        --keep)      KEEP_TESTER=1; shift ;;
        -h|--help)   usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

log() { printf '[memory_probe] %s\n' "$*" >&2; }
die() { log "FATAL: $*"; exit 1; }

command -v jq >/dev/null || die "jq is required"
[[ -x "$KCODE_BIN" ]] || die "jcode CLI not found at $KCODE_BIN"
[[ -x "$CLIENT_BIN" ]] || CLIENT_BIN="$(command -v jcode)" || die "client binary not found"
[[ -f "$HOME/.kcode/sessions/${SESSION_ID}.json" ]] \
    || log "WARN: $HOME/.kcode/sessions/${SESSION_ID}.json not found; resume may create a new session"

RUN_ID="probe_$(date -u +%Y%m%dT%H%M%SZ)_$$"
SESSION_FILE="$HOME/.kcode/sessions/${SESSION_ID}.json"
SESSION_BYTES=$(stat -c %s "$SESSION_FILE" 2>/dev/null || echo 0)

TESTER_ID=""
TESTER_PID=""
CMD_PATH=""
RESP_PATH=""
WRAPPER=""
SPAWN_EPOCH_MS=""

cleanup() {
    local rc=$?
    if [[ -n "$TESTER_ID" && "$KEEP_TESTER" -ne 1 ]]; then
        "$KCODE_BIN" debug "tester:${TESTER_ID}:stop" >/dev/null 2>&1 || true
        log "stopped tester $TESTER_ID"
    elif [[ -n "$TESTER_ID" ]]; then
        log "kept tester $TESTER_ID (pid $TESTER_PID) running (--keep)"
    fi
    [[ -n "$WRAPPER" ]] && rm -f "$WRAPPER"
    exit $rc
}
trap cleanup EXIT INT TERM

now_ms() { date +%s%3N; }

# ---------------------------------------------------------------------------
# /proc sampling: emits a JSON object with kB values from status+smaps_rollup.
# ---------------------------------------------------------------------------
proc_kb() { # file key
    awk -v k="$2" '$1 == k ":" { print $2; found=1; exit } END { if (!found) print "null" }' "$1" 2>/dev/null || echo null
}

sample_proc() { # pid -> json on stdout
    local pid="$1"
    local status="/proc/$pid/status" rollup="/proc/$pid/smaps_rollup"
    [[ -r "$status" ]] || { echo null; return 1; }
    local tmp_rollup
    tmp_rollup="$(mktemp)"
    # snapshot smaps_rollup once so all fields come from the same read
    cat "$rollup" > "$tmp_rollup" 2>/dev/null || true
    jq -n -S \
        --argjson rss_kb "$(proc_kb "$status" VmRSS)" \
        --argjson rss_anon_kb "$(proc_kb "$status" RssAnon)" \
        --argjson rss_file_kb "$(proc_kb "$status" RssFile)" \
        --argjson rss_shmem_kb "$(proc_kb "$status" RssShmem)" \
        --argjson vm_hwm_kb "$(proc_kb "$status" VmHWM)" \
        --argjson vm_swap_kb "$(proc_kb "$status" VmSwap)" \
        --argjson pss_kb "$(proc_kb "$tmp_rollup" Pss)" \
        --argjson pss_anon_kb "$(proc_kb "$tmp_rollup" Pss_Anon)" \
        --argjson pss_file_kb "$(proc_kb "$tmp_rollup" Pss_File)" \
        --argjson private_clean_kb "$(proc_kb "$tmp_rollup" Private_Clean)" \
        --argjson private_dirty_kb "$(proc_kb "$tmp_rollup" Private_Dirty)" \
        --argjson shared_clean_kb "$(proc_kb "$tmp_rollup" Shared_Clean)" \
        --argjson shared_dirty_kb "$(proc_kb "$tmp_rollup" Shared_Dirty)" \
        --argjson swap_kb "$(proc_kb "$tmp_rollup" Swap)" \
        '{rss_kb: $rss_kb, rss_anon_kb: $rss_anon_kb, rss_file_kb: $rss_file_kb,
          rss_shmem_kb: $rss_shmem_kb, vm_hwm_kb: $vm_hwm_kb, vm_swap_kb: $vm_swap_kb,
          pss_kb: $pss_kb, pss_anon_kb: $pss_anon_kb, pss_file_kb: $pss_file_kb,
          private_clean_kb: $private_clean_kb, private_dirty_kb: $private_dirty_kb,
          shared_clean_kb: $shared_clean_kb, shared_dirty_kb: $shared_dirty_kb,
          swap_kb: $swap_kb}'
    local rc=$?
    rm -f "$tmp_rollup"
    return $rc
}

# ---------------------------------------------------------------------------
# Tester file-based debug channel: write command, poll response until it is
# non-empty and stable (guards against partially written responses).
# ---------------------------------------------------------------------------
tester_cmd() { # command timeout_secs -> response on stdout, rc 1 on timeout
    local cmd="$1" timeout_s="${2:-30}"
    rm -f "$RESP_PATH"
    printf '%s' "$cmd" > "$CMD_PATH"
    local deadline=$(( $(date +%s) + timeout_s ))
    local prev="" cur=""
    while (( $(date +%s) < deadline )); do
        if [[ -s "$RESP_PATH" ]]; then
            cur="$(cat "$RESP_PATH" 2>/dev/null || true)"
            if [[ -n "$cur" && "$cur" == "$prev" ]]; then
                rm -f "$RESP_PATH"
                printf '%s' "$cur"
                return 0
            fi
            prev="$cur"
        fi
        sleep 0.2
    done
    return 1
}

tester_json_cmd() { # command timeout_secs -> valid JSON on stdout or rc 1
    local out
    out="$(tester_cmd "$1" "${2:-30}")" || return 1
    jq -e . >/dev/null 2>&1 <<<"$out" || return 1
    printf '%s' "$out"
}

# Client aggregate memory profile subset (same handler as `client:memory`).
client_memory_subset() { # timeout -> compact json or "null"
    local raw
    if ! raw="$(tester_json_cmd "memory" "${1:-90}")"; then
        echo null
        return 0
    fi
    jq -c -S '{
        rss_bytes: (.process.rss_bytes // null),
        pss_bytes: (.process.os.pss_bytes // null),
        rss_anon_bytes: (.process.os.rss_anon_bytes // null),
        allocator: (.process.allocator.name // null),
        allocated_bytes: (.process.allocator.stats.allocated_bytes // null),
        session_json_bytes: (.session.totals.json_bytes // null),
        provider_messages_count: (.ui.provider_messages.count // null),
        provider_messages_json_bytes: (.ui.provider_messages.json_bytes // null),
        display_messages_count: (.ui.display_messages.count // null),
        display_messages_estimate_bytes: (.ui.display_messages.estimate_bytes // null)
    }' <<<"$raw" 2>/dev/null || echo null
}

# ---------------------------------------------------------------------------
# Phase emitter: one compact key-sorted JSON line on stdout.
# ---------------------------------------------------------------------------
emit_phase() { # phase proc_json client_json extras_json
    local phase="$1" proc_json="$2" client_json="$3" extras_json="${4:-{\}}"
    local ts elapsed_ms
    ts="$(now_ms)"
    elapsed_ms=$(( ts - SPAWN_EPOCH_MS ))
    jq -c -S -n \
        --arg probe "jcode_memory_probe" \
        --arg schema "1" \
        --arg run_id "$RUN_ID" \
        --arg phase "$phase" \
        --arg session "$SESSION_ID" \
        --arg tester_id "$TESTER_ID" \
        --argjson session_file_bytes "$SESSION_BYTES" \
        --argjson pid "${TESTER_PID:-null}" \
        --argjson ts_ms "$ts" \
        --argjson elapsed_ms "$elapsed_ms" \
        --argjson proc "$proc_json" \
        --argjson client "$client_json" \
        --argjson extras "$extras_json" \
        '{probe: $probe, schema: ($schema | tonumber), run_id: $run_id, phase: $phase,
          session: $session, session_file_bytes: $session_file_bytes,
          tester_id: $tester_id, pid: $pid, ts_ms: $ts_ms, elapsed_ms: $elapsed_ms,
          proc: $proc, client: $client} + $extras'
}

# ---------------------------------------------------------------------------
# Forced trim: call malloc_trim(0) inside the client via gdb.
# ---------------------------------------------------------------------------
force_trim() { # pid -> prints method used ("gdb_sudo" | "gdb" | "unavailable")
    local pid="$1"
    if ! command -v gdb >/dev/null; then
        echo unavailable
        return 0
    fi
    local gdb_args=(--batch -p "$pid" -ex 'call (int) malloc_trim(0)' -ex detach -ex quit)
    if sudo -n true 2>/dev/null; then
        if timeout 60 sudo -n gdb "${gdb_args[@]}" >/dev/null 2>&1; then
            echo gdb_sudo
            return 0
        fi
    fi
    if timeout 60 gdb "${gdb_args[@]}" >/dev/null 2>&1; then
        echo gdb
        return 0
    fi
    echo unavailable
}

# ===========================================================================
# 1. Spawn headless tester resuming the target session.
# ===========================================================================
WRAPPER="$(mktemp /tmp/jcode_memory_probe_wrapper.XXXXXX.sh)"
cat > "$WRAPPER" <<EOF
#!/usr/bin/env bash
exec "$CLIENT_BIN" --resume "$SESSION_ID" --no-update "\$@"
EOF
chmod +x "$WRAPPER"

log "spawning headless tester (binary wrapper resumes $SESSION_ID, ${SESSION_BYTES} byte session file)"
SPAWN_OUT="$("$KCODE_BIN" debug "tester:spawn {\"binary\":\"$WRAPPER\",\"cwd\":\"$TESTER_CWD\",\"cols\":120,\"rows\":40}")" \
    || die "tester:spawn failed: $SPAWN_OUT"
SPAWN_EPOCH_MS="$(now_ms)"
TESTER_ID="$(jq -r '.id // empty' <<<"$SPAWN_OUT")"
TESTER_PID="$(jq -r '.pid // empty' <<<"$SPAWN_OUT")"
[[ -n "$TESTER_ID" && -n "$TESTER_PID" ]] || die "could not parse tester:spawn response: $SPAWN_OUT"
log "tester $TESTER_ID pid $TESTER_PID"

TESTER_INFO="$("$KCODE_BIN" debug tester:list | jq -c --arg id "$TESTER_ID" '.[] | select(.id == $id)')"
CMD_PATH="$(jq -r '.debug_cmd_path' <<<"$TESTER_INFO")"
RESP_PATH="$(jq -r '.debug_response_path' <<<"$TESTER_INFO")"
[[ -n "$CMD_PATH" && -n "$RESP_PATH" ]] || die "could not resolve tester debug paths"

# ===========================================================================
# 2. Phase: fresh (immediately after spawn, before the event loop settles).
# ===========================================================================
sleep 0.2
PROC_JSON="$(sample_proc "$TESTER_PID")" || die "tester process $TESTER_PID vanished during fresh phase"
emit_phase "fresh" "$PROC_JSON" null '{}'

# ===========================================================================
# 3. Phase: post_connect (first successful client debug state response).
# ===========================================================================
STATE_JSON=""
CONNECT_DEADLINE=$(( $(date +%s) + 60 ))
while (( $(date +%s) < CONNECT_DEADLINE )); do
    if STATE_JSON="$(tester_json_cmd "state" 5)"; then
        break
    fi
    STATE_JSON=""
done
[[ -n "$STATE_JSON" ]] || die "tester never answered a state command (stderr: $(jq -r '.stderr_path' <<<"$TESTER_INFO"))"
PROC_JSON="$(sample_proc "$TESTER_PID")"
DISPLAY_COUNT="$(jq -r '.display_messages // 0' <<<"$STATE_JSON")"
emit_phase "post_connect" "$PROC_JSON" null "{\"display_messages\": $DISPLAY_COUNT}"
log "connected; display_messages=$DISPLAY_COUNT"

# ===========================================================================
# 4. Phase: post_history_loaded (display messages present and stable).
# ===========================================================================
HISTORY_DEADLINE=$(( $(date +%s) + 180 ))
PREV_COUNT=-1
STABLE=0
while (( $(date +%s) < HISTORY_DEADLINE )); do
    if STATE_JSON="$(tester_json_cmd "state" 10)"; then
        DISPLAY_COUNT="$(jq -r '.display_messages // 0' <<<"$STATE_JSON")"
        PROCESSING="$(jq -r '.processing // false' <<<"$STATE_JSON")"
        if [[ "$DISPLAY_COUNT" -gt 0 && "$PROCESSING" == "false" && "$DISPLAY_COUNT" -eq "$PREV_COUNT" ]]; then
            STABLE=$(( STABLE + 1 ))
            [[ $STABLE -ge 2 ]] && break
        else
            STABLE=0
        fi
        PREV_COUNT="$DISPLAY_COUNT"
    fi
    sleep 1
done
[[ "$PREV_COUNT" -gt 0 ]] || die "history never loaded (display_messages=$PREV_COUNT)"
PROC_JSON="$(sample_proc "$TESTER_PID")"
CLIENT_JSON="$(client_memory_subset 90)"
emit_phase "post_history_loaded" "$PROC_JSON" "$CLIENT_JSON" "{\"display_messages\": $PREV_COUNT}"
log "history loaded; display_messages=$PREV_COUNT"

# ===========================================================================
# 5. Phase: idle (after --idle-secs of no activity).
# ===========================================================================
log "idling for ${IDLE_SECS}s"
sleep "$IDLE_SECS"
PROC_JSON="$(sample_proc "$TESTER_PID")"
CLIENT_JSON="$(client_memory_subset 90)"
emit_phase "idle" "$PROC_JSON" "$CLIENT_JSON" "{\"idle_secs\": $IDLE_SECS}"

# ===========================================================================
# 6. Phase: post_trim (forced allocator trim via gdb malloc_trim(0)).
# ===========================================================================
if [[ "$SKIP_TRIM" -eq 1 ]]; then
    log "skipping trim phase (--skip-trim)"
else
    TRIM_METHOD="$(force_trim "$TESTER_PID")"
    log "forced trim via: $TRIM_METHOD"
    sleep 1
    PROC_JSON="$(sample_proc "$TESTER_PID")"
    CLIENT_JSON="$(client_memory_subset 90)"
    emit_phase "post_trim" "$PROC_JSON" "$CLIENT_JSON" "{\"trim_method\": \"$TRIM_METHOD\"}"
fi

log "done (run_id=$RUN_ID)"
