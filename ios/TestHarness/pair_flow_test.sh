#!/usr/bin/env bash
#
# One-command pairing flow test: starting from a machine with the gateway
# disabled and no server running, `jcode pair` alone must enable the gateway,
# start a server that answers /health, print a code, and confirm when a phone
# pairs. Runs in a throwaway JCODE_HOME so it never touches the real install.
#
# Usage: ./TestHarness/pair_flow_test.sh [--port 7655]
#   JCODE_BIN=/path/to/jcode selects the binary (default: target/selfdev/jcode).
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$(cd ../ && pwd)"
PORT=7655
[[ "${1:-}" == "--port" ]] && PORT="$2"
JCODE_BIN="${JCODE_BIN:-$REPO_ROOT/target/selfdev/jcode}"
OUT_DIR="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/jcode-pair-flow"
HOME_DIR="$OUT_DIR/home"
mkdir -p "$HOME_DIR/run"
echo '{}' > "$HOME_DIR/devices.json"
printf '[gateway]\nenabled = false\nport = %s\nbind_addr = "0.0.0.0"\n\n[ambient]\nenabled = false\n' \
  "$PORT" > "$HOME_DIR/config.toml"

export JCODE_HOME="$HOME_DIR" JCODE_RUNTIME_DIR="$HOME_DIR/run" \
  JCODE_SOCKET="$HOME_DIR/run/jcode.sock" JCODE_NO_TELEMETRY=1

FAILED=0
pass() { printf '\033[32m  ok\033[0m %s\n' "$*"; }
fail() { printf '\033[31m  FAIL\033[0m %s\n' "$*"; FAILED=1; }

cleanup() {
  # Only kill servers whose environment points at this sandbox, never the
  # developer's own daemon.
  for pid in $(pgrep -f "jcode.* serve" 2>/dev/null); do
    if ps eww -p "$pid" 2>/dev/null | grep -q "JCODE_HOME=$HOME_DIR"; then
      kill "$pid" 2>/dev/null || true
    fi
  done
}
trap cleanup EXIT
cleanup

# Scenario: a server is already running with the gateway off (the usual state
# for someone who has been using jcode and just installed the app). `jcode
# pair` must reload it rather than print a code nobody can reach.
if [[ "${PREEXISTING_SERVER:-1}" == "1" ]]; then
  "$JCODE_BIN" serve --no-update > "$OUT_DIR/preexisting-server.log" 2>&1 &
  for _ in $(seq 1 120); do
    # A socket file can be left over from a previous run; require a listener.
    python3 -c "import socket,sys;s=socket.socket(socket.AF_UNIX);s.settimeout(1);s.connect(sys.argv[1])" \
      "$HOME_DIR/run/jcode.sock" 2>/dev/null && break
    sleep 0.5
  done
  if curl -fsS --max-time 1 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
    fail "precondition: gateway should be off before pairing"
  else
    pass "precondition: server running, gateway off"
  fi
fi

# Simulated phone: waits for a code, then pairs over the real HTTP endpoint.
(
  for _ in $(seq 1 60); do
    CODE="$(python3 -c "import json;c=json.load(open('$HOME_DIR/devices.json')).get('pending_codes') or [];print(c[-1]['code'] if c else '')" 2>/dev/null || true)"
    if [[ -n "$CODE" ]] && curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
      sleep 1
      curl -fsS -X POST "http://127.0.0.1:$PORT/pair" -H 'Content-Type: application/json' \
        -d "{\"code\":\"$CODE\",\"device_id\":\"flow-phone\",\"device_name\":\"Flow Test iPhone\"}" \
        > "$OUT_DIR/pair-response.json" 2>/dev/null || true
      exit 0
    fi
    sleep 1
  done
) &
PHONE_PID=$!

# `script` gives the command a TTY so the interactive wait-for-phone path runs.
# The daemon it spawns inherits the pty, so `script` itself lingers after
# `jcode pair` exits; finish as soon as pair reports the outcome.
script -q /dev/null "$JCODE_BIN" pair --no-update > "$OUT_DIR/pair.out" 2>&1 &
PAIR_PID=$!
for _ in $(seq 1 240); do
  grep -qE "Paired |expired|not answering" "$OUT_DIR/pair.out" 2>/dev/null && break
  sleep 0.5
done
kill "$PAIR_PID" 2>/dev/null || true
wait "$PHONE_PID" 2>/dev/null || true
CLEAN="$(sed 's/\x1b\[[0-9;]*m//g' "$OUT_DIR/pair.out" | tr -d '\r')"
printf '%s\n' "$CLEAN" | grep -v '[█▀▄]' | sed 's/^/    /'

grep -q '^enabled = true' "$HOME_DIR/config.toml" && pass "gateway enabled in config" \
  || fail "gateway still disabled in config"
printf '%s' "$CLEAN" | grep -q "Gateway listening on port $PORT" && pass "server started with gateway" \
  || fail "pair did not bring the gateway up"
printf '%s' "$CLEAN" | grep -q "Pairing code:" && pass "pairing code shown" || fail "no pairing code"
printf '%s' "$CLEAN" | grep -q "Paired Flow Test iPhone" && pass "pair confirmed the phone" \
  || fail "pair never confirmed the phone"
grep -q '"token"' "$OUT_DIR/pair-response.json" 2>/dev/null && pass "phone received a token" \
  || fail "phone pairing failed"
curl -fsS "http://127.0.0.1:$PORT/health" | grep -q working_dir && pass "/health advertises working_dir" \
  || fail "/health missing working_dir"

if [[ $FAILED -eq 0 ]]; then
  printf '\033[32mPAIR FLOW PASSED\033[0m\n'
else
  printf '\033[31mPAIR FLOW FAILED\033[0m\n'; exit 1
fi
