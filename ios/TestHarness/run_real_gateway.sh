#!/usr/bin/env bash
#
# Real-gateway harness: runs the actual jcode server with its WebSocket gateway
# enabled, pairs the iOS app against it over the real POST /pair flow, sends a
# real prompt, and asserts a live model turn renders in the simulator.
#
# The mock gateway (run_e2e.sh) proves the client handles a *scripted* protocol.
# This proves the client works against the shipping server: real handshake,
# real auth, real streaming, real model output. Anything that only the mock
# tolerates shows up here.
#
# Isolation: the server runs under a throwaway JCODE_HOME so it never touches
# the developer's sessions, devices.json, or socket. Credentials are inherited
# by copying auth.json into the sandbox (read-only usage, never written back).
#
# Usage:
#   ./TestHarness/run_real_gateway.sh [--device "iPhone 17"] [--port 7644]
#                                     [--prompt "..."] [--keep] [--no-build]
#
set -euo pipefail

cd "$(dirname "$0")/.."        # ios/
REPO_ROOT="$(cd ../ && pwd)"
BUNDLE_ID="com.jcode.mobile"
DEVICE="iPhone 17"
PORT=7644
PROMPT="Reply with exactly: REAL_GATEWAY_OK"
EXPECT="REAL_GATEWAY_OK"
KEEP=""
NO_BUILD=""
OUT_DIR="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/jcode-ios-real"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --prompt) PROMPT="$2"; shift 2 ;;
    --expect) EXPECT="$2"; shift 2 ;;
    --keep) KEEP="1"; shift ;;
    --no-build) NO_BUILD="1"; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"
SANDBOX="$OUT_DIR/home"
mkdir -p "$SANDBOX"

log()  { printf '\033[36m[real]\033[0m %s\n' "$*"; }
pass() { printf '\033[32m  ok\033[0m %s\n' "$*"; }
fail() { printf '\033[31m  FAIL\033[0m %s\n' "$*"; FAILED=1; }
FAILED=0

SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [[ -z "$KEEP" ]]; then
    rm -rf "$SANDBOX"
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 1. Locate a jcode binary. Prefer the current self-dev build.
# ---------------------------------------------------------------------------
JCODE_BIN=""
# Prefer a freshly built binary from this worktree over the installed one so
# server-side changes are exercised by the same run that tests the client.
for cand in \
  "$REPO_ROOT/target/selfdev/jcode" \
  "$HOME/.jcode/builds/current/jcode" \
  "$REPO_ROOT/target/debug/jcode" \
  "$(command -v jcode || true)"
do
  if [[ -n "$cand" && -x "$cand" ]]; then JCODE_BIN="$cand"; break; fi
done
if [[ -z "$JCODE_BIN" ]]; then
  echo "no jcode binary found (build one, or install to ~/.jcode/builds/current)" >&2
  exit 2
fi
log "server binary: $JCODE_BIN ($("$JCODE_BIN" --version 2>/dev/null | head -1))"

# ---------------------------------------------------------------------------
# 2. Build an isolated JCODE_HOME with the gateway enabled.
#    Auth is copied so a real provider turn can run; sessions/devices are not.
# ---------------------------------------------------------------------------
# Providers keep credentials in several files (auth.json plus per-provider
# ones like openai-auth.json), so copy them all or the live turn has no key.
for f in auth.json auth-refresh-state.json auth-validation.json \
         openai-auth.json gemini-auth.json copilot-auth.json \
         cursor-auth.json antigravity-auth.json; do
  [[ -f "$HOME/.jcode/$f" ]] && cp "$HOME/.jcode/$f" "$SANDBOX/$f"
done
# Some providers read credentials from ~/.jcode/external/.
if [[ -d "$HOME/.jcode/external" ]]; then
  cp -R "$HOME/.jcode/external" "$SANDBOX/external" 2>/dev/null || true
fi
MODEL_LINE="$(grep -E '^default_model|^default_provider' "$HOME/.jcode/config.toml" 2>/dev/null || true)"
{
  echo "[gateway]"
  echo "enabled = true"
  echo "port = $PORT"
  echo 'bind_addr = "127.0.0.1"'
  echo
  echo "[provider]"
  if [[ -n "$MODEL_LINE" ]]; then echo "$MODEL_LINE"; fi
  echo
  # Keep the turn cheap and deterministic: no tools, no ambient work.
  echo "[ambient]"
  echo "enabled = false"
} > "$SANDBOX/config.toml"
log "sandbox home: $SANDBOX (gateway 127.0.0.1:$PORT)"

# A still-running app from a previous run reconnects to this port the moment the
# server comes up and can take over the session the probe is using, so stop it
# before anything else. (Harmless when nothing is running.)
xcrun simctl terminate "$DEVICE" "$BUNDLE_ID" 2>/dev/null || true

export JCODE_HOME="$SANDBOX"
export JCODE_RUNTIME_DIR="$SANDBOX/run"
export JCODE_GATEWAY_ENABLED=1
export JCODE_GATEWAY_PORT="$PORT"
export JCODE_GATEWAY_BIND_ADDR=127.0.0.1
export JCODE_NON_INTERACTIVE=1
export JCODE_NO_TELEMETRY=1
mkdir -p "$JCODE_RUNTIME_DIR"
# Pin the socket explicitly: runtime_dir() falls back to $TMPDIR on macOS, and
# reusing the developer's live socket would make the server refuse to start.
export JCODE_SOCKET="$JCODE_RUNTIME_DIR/jcode.sock"

# ---------------------------------------------------------------------------
# 3. Start the real server and wait for the gateway to answer /health.
# ---------------------------------------------------------------------------
log "starting real jcode server"
"$JCODE_BIN" serve --no-update --server-name ios-real-harness \
  >"$OUT_DIR/server.log" 2>&1 &
SERVER_PID=$!

HEALTH=""
for _ in $(seq 1 60); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "server exited early; log:" >&2; tail -30 "$OUT_DIR/server.log" >&2; exit 1
  fi
  HEALTH="$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" 2>/dev/null || true)"
  [[ -n "$HEALTH" ]] && break
  sleep 1
done
if [[ -z "$HEALTH" ]]; then
  fail "gateway never answered /health on :$PORT"
  tail -40 "$OUT_DIR/server.log" >&2
  exit 1
fi
pass "GET /health -> $HEALTH"

# ---------------------------------------------------------------------------
# 4. Pair over the real POST /pair flow (code generated by the real registry).
# ---------------------------------------------------------------------------
log "pairing via real POST /pair"
"$JCODE_BIN" pair --no-update >"$OUT_DIR/pair.log" 2>&1 || true
CODE="$(python3 - "$SANDBOX/devices.json" <<'PY'
import json, sys
try:
    reg = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
codes = reg.get("pending_codes") or []
print(codes[-1]["code"] if codes else "")
PY
)"
if [[ -z "$CODE" ]]; then
  fail "no pairing code generated (see $OUT_DIR/pair.log)"
  exit 1
fi
PAIR_JSON="$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:$PORT/pair" \
  -H 'Content-Type: application/json' \
  -d "{\"code\":\"$CODE\",\"device_id\":\"ios-real-harness\",\"device_name\":\"Harness Simulator\"}" \
  2>/dev/null || true)"
TOKEN="$(printf '%s' "$PAIR_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("token",""))' 2>/dev/null || true)"
if [[ -z "$TOKEN" ]]; then
  fail "pairing failed: $PAIR_JSON"
  exit 1
fi
pass "paired, token ${TOKEN:0:8}... server=$(printf '%s' "$PAIR_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("server_name",""))')"

# ---------------------------------------------------------------------------
# 5. Protocol-level assertion over the real WebSocket: a live model turn.
#    Runs before the UI so a server/model failure is not misread as a UI bug.
# ---------------------------------------------------------------------------
log "live model turn over real WebSocket"
set +e
python3 "TestHarness/real_gateway_probe.py" \
  --port "$PORT" --token "$TOKEN" --prompt "$PROMPT" --expect "$EXPECT" \
  --json-out "$OUT_DIR/probe.json" | sed 's/^/  /'
PROBE_RC=$?
set -e
[[ $PROBE_RC -eq 0 ]] && pass "real model turn streamed and matched" || fail "live turn probe failed"

# ---------------------------------------------------------------------------
# 6. Drive the real app against the real gateway in the simulator.
# ---------------------------------------------------------------------------
if [[ -z "$NO_BUILD" ]]; then
  log "building app for $DEVICE"
  xcodegen generate >/dev/null
  xcodebuild build \
    -project JCodeMobile.xcodeproj -scheme JCodeMobile \
    -destination "platform=iOS Simulator,name=$DEVICE" \
    -derivedDataPath .build-ios >"$OUT_DIR/xcodebuild.log" 2>&1 \
    || { tail -30 "$OUT_DIR/xcodebuild.log" >&2; fail "app build failed"; exit 1; }
fi
APP=".build-ios/Build/Products/Debug-iphonesimulator/JCodeMobile.app"

log "booting simulator: $DEVICE"
xcrun simctl boot "$DEVICE" 2>/dev/null || true
sleep 3

log "installing app + seeding the real credential"
# The simulator keychain survives app uninstalls, so a token from a previous
# sandbox would outrank the freshly seeded file (KeychainCredentialStore reads
# the keychain first). Reset it so each run starts from a known state.
xcrun simctl keychain "$DEVICE" reset 2>/dev/null || true
xcrun simctl uninstall "$DEVICE" "$BUNDLE_ID" 2>/dev/null || true
xcrun simctl install "$DEVICE" "$APP"
CONTAINER="$(xcrun simctl get_app_container "$DEVICE" "$BUNDLE_ID" data)"
APPSUP="$CONTAINER/Library/Application Support"
mkdir -p "$APPSUP"
SERVER_NAME="$(printf '%s' "$PAIR_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("server_name","jcode"))')"
SERVER_VER="$(printf '%s' "$PAIR_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("server_version",""))')"
python3 - "$APPSUP/jcode-servers.json" "$PORT" "$TOKEN" "$SERVER_NAME" "$SERVER_VER" <<'PY'
import json, sys, time
path, port, token, name, ver = sys.argv[1:6]
json.dump([{
    "host": "127.0.0.1", "port": int(port), "token": token,
    "serverName": name, "serverVersion": ver,
    "pairedAt": int(time.time()),
}], open(path, "w"))
PY

log "launching app"
xcrun simctl launch "$DEVICE" "$BUNDLE_ID" >/dev/null
sleep 8
SHOT="$OUT_DIR/real-chat.png"
xcrun simctl io "$DEVICE" screenshot "$SHOT" >/dev/null 2>&1
pass "screenshot: $SHOT"

# Assert from the server's own log that the *app* (not just the probe) opened an
# authorized WebSocket. The server writes to $JCODE_HOME/logs, and stdout only
# carries the startup banner, so copy the real log out before the sandbox goes.
GW_LOG="$OUT_DIR/gateway.log"
cat "$SANDBOX/logs/"*.log > "$GW_LOG" 2>/dev/null || true

APP_CONNECTS="$(grep -c "Harness Simulator connected" "$GW_LOG" 2>/dev/null | tr -d ' ')"
APP_CONNECTS="${APP_CONNECTS:-0}"
UNAUTHORIZED="$(grep -c "401 Unauthorized" "$GW_LOG" 2>/dev/null | tr -d ' ')"
UNAUTHORIZED="${UNAUTHORIZED:-0}"

# The probe accounts for exactly one connection; anything beyond that is the app.
if [[ "$APP_CONNECTS" -ge 2 ]]; then
  pass "app opened an authorized WebSocket to the real server ($APP_CONNECTS connects)"
else
  fail "app never reached the real gateway (connects=$APP_CONNECTS)"
fi
if [[ "$UNAUTHORIZED" -eq 0 ]]; then
  pass "no 401s: the seeded credential authenticated"
else
  fail "$UNAUTHORIZED unauthorized upgrade(s) - stale token or keychain leak"
fi
if grep -q "Subscribe requires the client's working directory" "$GW_LOG"; then
  fail "server rejected a subscribe for a missing working_dir"
else
  pass "every subscribe carried a working_dir"
fi

log "gateway log: $GW_LOG"
if [[ $FAILED -eq 0 ]]; then
  printf '\033[32mREAL GATEWAY CHECKS PASSED\033[0m\n'
else
  printf '\033[31mREAL GATEWAY CHECKS FAILED\033[0m\n'
  exit 1
fi
