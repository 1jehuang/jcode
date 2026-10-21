#!/usr/bin/env bash
# test-portal-bridge.sh — verify mcp-remote accepts a local stub client
# metadata document and configures the matching OAuth callback.
#
# What this tests:
#   1. The stub CIMD server starts and serves /cimd.json over HTTPS
#   2. mcp-remote (pinned v0.15.1) ACCEPTS the client metadata URL
#      (i.e. it does not silently discard it) and begins OAuth discovery
#   3. mcp-remote uses the 127.0.0.1 callback host that the CIMD
#      registered, and derives a callback port
#
# Why HTTPS (fixed 2026-10-19):
#   mcp-remote's isClientMetadataUrl() requires an https: URL with a
#   non-root pathname. An http:// client metadata URL is SILENTLY IGNORED
#   ("Warning: Ignoring invalid client metadata URL"), so a plain-HTTP stub
#   meant mcp-remote never even considered the document the test claimed
#   to verify. The stub therefore serves a self-signed 127.0.0.1
#   certificate and the test points NODE_EXTRA_CA_CERTS at it (undici
#   honours that variable).
#
# Why the assertions are NOT "the stub logged a /cimd.json fetch" (fixed
# 2026-10-19):
#   mcp-remote does not download the client metadata document itself. Per
#   SEP-991 it uses the CIMD URL AS THE client_id and asks the
#   AUTHORIZATION SERVER to fetch it; mcp-remote only reads the AS
#   metadata to check client_id_metadata_document_supported === true. The
#   stub therefore never sees a /cimd.json request from mcp-remote. The
#   old assertion passed only because the harness's own curl populated the
#   same log line -- a false positive that hid the HTTP/HTTPS defect
#   entirely.
#
# What this does NOT test:
#   - Completing the OAuth flow (requires the real Spotify Portal)
#   - The real Cloudflare Worker serving the production CIMD
#   - That the AS accepts the client metadata document (needs the real AS)
#   - The allow-list gate (a Spotify/vendor action)
#
# Exit codes:
#   0 = bridge accepted the CIMD URL and configured the 127.0.0.1 callback
#   1 = test infrastructure failure (stub didn't start, no TLS material, the
#       bridge produced no output at all before the timeout, etc.)
#   2 = the bridge did not prove the client-metadata / callback configuration
#
# Bridge timeout (fixed 2026-10-21):
#   The bridge step was hard-coded to 30s. `npx -y mcp-remote@0.15.1` can
#   spend longer than that resolving the package on a cold npx cache, and it
#   writes nothing to stderr while it does. The bridge log was then empty and
#   the assertion below reported exit 2 ("the bridge did not prove the
#   configuration") when in reality the harness never gave the bridge a
#   chance to prove anything. Observed on 2026-10-21: one cold run failed with
#   a 0-byte bridge log, then three warm runs passed in 28s / 19s / 15s, so
#   30s sat right on the boundary. The timeout is now configurable
#   (BRIDGE_TIMEOUT, default 120s) and an empty bridge log is classified as an
#   infrastructure failure (exit 1) instead of a false assertion failure.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
STUB_SCRIPT="$SCRIPT_DIR/stub-cimd-server.py"
WORK_DIR=$(mktemp -d)
TLS_DIR="$WORK_DIR/tls"
STUB_LOG="$WORK_DIR/stub.log"
BRIDGE_LOG="$WORK_DIR/bridge.log"
INIT_LOG="$WORK_DIR/init.log"
STUB_PID=""
# Cold `npx` resolution can take well over 30s and writes nothing meanwhile.
BRIDGE_TIMEOUT="${BRIDGE_TIMEOUT:-120}"
# GNU timeout treats 0 as "no deadline" and a non-integer is a usage error, so a
# stray BRIDGE_TIMEOUT could either hang forever or fail opaquely. Require a
# positive integer and fall back loudly rather than silently changing meaning.
if ! [[ "$BRIDGE_TIMEOUT" =~ ^[0-9]+$ ]] || (( BRIDGE_TIMEOUT < 1 )); then
    echo "WARNING: BRIDGE_TIMEOUT='$BRIDGE_TIMEOUT' is not a positive integer; using 120s." >&2
    BRIDGE_TIMEOUT=120
fi

cleanup() {
    [[ -n "$STUB_PID" ]] && kill "$STUB_PID" 2>/dev/null || true
    [[ -n "$STUB_PID" ]] && wait "$STUB_PID" 2>/dev/null || true
    # Only keep logs if test failed
    if [[ "${TEST_FAILED:-0}" == "1" ]]; then
        echo "--- stub log ---"
        cat "$STUB_LOG" 2>/dev/null || true
        echo "--- bridge log ---"
        cat "$BRIDGE_LOG" 2>/dev/null || true
        echo "--- init log ---"
        cat "$INIT_LOG" 2>/dev/null || true
        echo "---"
        echo "Working dir (preserved for debugging): $WORK_DIR"
    else
        rm -rf "$WORK_DIR"
    fi
}
trap cleanup EXIT

TEST_FAILED=1

# Generate a short-lived self-signed cert for 127.0.0.1 so the stub can serve
# HTTPS. Scoped to the temp work dir; never written into the repo.
mkdir -p "$TLS_DIR"
if ! openssl req -x509 -newkey rsa:2048 -keyout "$TLS_DIR/key.pem" -out "$TLS_DIR/cert.pem" \
    -days 1 -nodes -subj "/CN=127.0.0.1" \
    -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" >/dev/null 2>&1; then
    echo "FAIL: could not generate a self-signed certificate for the stub."
    echo "      openssl is required for this test (HTTPS is mandatory because"
    echo "      mcp-remote ignores http:// client metadata URLs)."
    exit 1
fi

echo "=== XJ-03 Portal Bridge Local Test ==="
echo ""

# Step 1: Start stub CIMD server
echo "[1/5] Starting stub CIMD server..."
python3 "$STUB_SCRIPT" --tls-dir "$TLS_DIR" > "$WORK_DIR/stub-port.txt" 2>"$STUB_LOG" &
STUB_PID=$!

# Wait for the stub to print its port (first line to stdout)
for i in $(seq 1 10); do
    if [[ -s "$WORK_DIR/stub-port.txt" ]] && [[ "$(wc -l < "$WORK_DIR/stub-port.txt")" -ge 2 ]]; then
        break
    fi
    sleep 0.5
done

if ! kill -0 "$STUB_PID" 2>/dev/null; then
    echo "FAIL: Stub server exited prematurely"
    cat "$STUB_LOG" 2>/dev/null || true
    exit 1
fi

STUB_PORT=$(head -1 "$WORK_DIR/stub-port.txt" | tr -d '[:space:]')
if [[ -z "$STUB_PORT" ]]; then
    echo "FAIL: Stub server did not report its port"
    cat "$STUB_LOG" 2>/dev/null || true
    exit 1
fi

STUB_SCHEME=$(sed -n '2p' "$WORK_DIR/stub-port.txt" | sed 's/^scheme=//')
if [[ "$STUB_SCHEME" != "https" ]]; then
    echo "FAIL: Stub is serving PLAIN HTTP. mcp-remote's isClientMetadataUrl()"
    echo "      requires https: and silently ignores an http:// client metadata"
    echo "      URL, so the CIMD fetch assertion below would be meaningless."
    cat "$STUB_LOG" 2>/dev/null || true
    exit 1
fi

CIMD_URL="https://127.0.0.1:${STUB_PORT}/cimd.json"
MCP_URL="https://127.0.0.1:${STUB_PORT}"

echo "  Stub PID  : $STUB_PID"
echo "  Stub port : $STUB_PORT"
echo "  Stub TLS  : $STUB_SCHEME (self-signed, trusted via NODE_EXTRA_CA_CERTS)"
echo "  CIMD URL  : $CIMD_URL"
echo "  MCP URL   : $MCP_URL"

# Step 2: Verify stub serves CIMD
echo ""
echo "[2/5] Verifying stub serves /cimd.json..."
CIMD_RESPONSE=$(curl -sS --cacert "$TLS_DIR/cert.pem" "$CIMD_URL" 2>&1)
CIMD_STATUS=$(curl -sS --cacert "$TLS_DIR/cert.pem" -o /dev/null -w "%{http_code}" "$CIMD_URL" 2>&1)
CIMD_CT=$(curl -sS --cacert "$TLS_DIR/cert.pem" -I "$CIMD_URL" 2>&1 | grep -i content-type | tr -d '\r')

if [[ "$CIMD_STATUS" != "200" ]]; then
    echo "FAIL: Stub returned HTTP $CIMD_STATUS for /cimd.json"
    exit 1
fi
echo "  HTTP $CIMD_STATUS — $CIMD_CT"
echo "  Body: $(echo "$CIMD_RESPONSE" | python3 -c 'import sys,json; d=json.load(sys.stdin); print("client_id=" + str(d["client_id"]) + ", client_name=" + str(d["client_name"]))' 2>/dev/null || echo '(parse error)')"

# The harness's own probe above produced /cimd.json requests in the stub
# log. The count is reported in step 5 for transparency, but it is NOT an
# assertion: mcp-remote does not download the client metadata document
# itself (the authorization server does, per SEP-991), so the only local
# requests are the harness's own.
#
# (Corrected 2026-10-19. The previous version of this test asserted that
# the stub logged a /cimd.json fetch caused by mcp-remote. That was a false
# positive: the only /cimd.json request in the stub log came from the
# harness's OWN curl. Combined with a plain-HTTP stub -- which mcp-remote
# silently ignores, since isClientMetadataUrl() requires https: -- the
# assertion was vacuous.)
BASELINE_CIMD_FETCHES=$(grep -c "200 /cimd.json" "$STUB_LOG" 2>/dev/null || echo 0)
echo "  Baseline CIMD fetches (harness probe only): $BASELINE_CIMD_FETCHES"

# Step 3: Start mcp-remote bridge
echo ""
echo "[3/5] Starting mcp-remote bridge (v0.15.1)..."
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test-harness","version":"0.1.0"}}}' > "$WORK_DIR/init-request.json"

# Run mcp-remote: pipe the init request, capture output, with a timeout.
#
# What mcp-remote actually does with --client-metadata-url (SEP-991):
#   It does NOT download the document itself. It uses the CIMD URL AS THE
#   client_id and asks the AUTHORIZATION SERVER to fetch it. mcp-remote
#   only reads the AS metadata to check
#   client_id_metadata_document_supported === true. So the local stub is
#   never hit by mcp-remote for /cimd.json -- the AS would do that.
#
#   (Corrected 2026-10-19. The previous version of this test asserted that
#   the stub logged a /cimd.json fetch caused by mcp-remote. That was a
#   false positive: the only /cimd.json request in the stub log came from
#   the harness's OWN curl in step 2. Combined with a plain-HTTP stub --
#   which mcp-remote silently ignores, since isClientMetadataUrl() requires
#   https: -- the assertion was vacuous.)
#
# What this test CAN prove locally:
#   a. mcp-remote ACCEPTS the CIMD URL (did not log "Ignoring invalid
#      client metadata URL") -- this is the real regression guard, because
#      an http:// URL is silently dropped and the bridge would fall back to
#      dynamic registration with a localhost redirect_uri.
#   b. mcp-remote uses --host 127.0.0.1 as the callback hostname.
#   c. mcp-remote derives a callback port and starts the callback server.
#
# Arg order: the server URL must be the FIRST positional arg. Putting
# --transport before it makes mcp-remote parse '--transport' as the URL
# (TypeError: Invalid URL). Fixed 2026-10-19.
# --host 127.0.0.1 must match the redirect_uri registered in the CIMD
# (the stub publishes 127.0.0.1; mcp-remote defaults to localhost on
# non-Windows). Fixed 2026-10-19.
# Capture the exit status. 124 is GNU timeout's "deadline reached" code. It is
# NOT swallowed: a timed-out bridge is an infrastructure failure even when it
# managed to write partial stderr before being killed, and without this the
# assertions below could mistake a truncated log for a real CIMD rejection.
BRIDGE_RC=0
timeout "$BRIDGE_TIMEOUT" bash -c "
    cat '$WORK_DIR/init-request.json' | \
    NODE_EXTRA_CA_CERTS='$TLS_DIR/cert.pem' \
    npx -y mcp-remote@0.15.1 \
        '$MCP_URL' \
        --transport http-only \
        --client-metadata-url '$CIMD_URL' \
        --resource '$MCP_URL' \
        --host 127.0.0.1 \
        --debug \
        2>'$BRIDGE_LOG' > '$INIT_LOG'
" || BRIDGE_RC=$?

echo "  Bridge exited (expected — no real MCP server or OAuth AS)"

if (( BRIDGE_RC == 124 )); then
    echo "FAIL: the bridge hit the ${BRIDGE_TIMEOUT}s timeout (exit 124)."
    echo "      This is an infrastructure failure, not a CIMD rejection: a cold"
    echo "      \`npx -y mcp-remote@0.15.1\` resolve writes nothing to stderr while"
    echo "      it downloads, so a truncated log is not evidence about the CIMD."
    echo "      Retry, or raise the budget: BRIDGE_TIMEOUT=300 $0"
    exit 1
fi

# An empty bridge log means the harness never observed anything: a cold npx
# resolve, a crash before the first write, or a timeout kill. That is an
# infrastructure failure (exit 1), NOT evidence that the bridge rejected the
# client metadata document (exit 2). Classify it explicitly so a flaky cold
# start cannot masquerade as a real regression.
if [[ ! -s "$BRIDGE_LOG" ]]; then
    echo "FAIL: the bridge produced no output within ${BRIDGE_TIMEOUT}s."
    echo "      This is an infrastructure failure, not a CIMD rejection:"
    echo "      a cold \`npx -y mcp-remote@0.15.1\` resolve can exceed the timeout"
    echo "      and writes nothing to stderr while it downloads."
    echo "      Retry, or raise the budget: BRIDGE_TIMEOUT=300 $0"
    exit 1
fi

# mcp-remote must ACCEPT the client metadata document. If it ignored it
# (e.g. an http:// URL), the whole test would be vacuous.
CIMD_ACCEPTED=0
if grep -qi "Ignoring invalid client metadata" "$BRIDGE_LOG" 2>/dev/null; then
    echo "FAIL: mcp-remote IGNORED the client metadata document:"
    grep -i "Ignoring invalid client metadata" "$BRIDGE_LOG" | head -3 | sed 's/^/    /'
    exit 2
fi
if ! grep -qi "Using client metadata document" "$BRIDGE_LOG" 2>/dev/null; then
    echo "FAIL: mcp-remote never logged that it accepted the client metadata document."
    exit 2
fi
CIMD_ACCEPTED=1
echo "  CONFIRMED: mcp-remote accepted the client metadata document"

# Step 4: Check bridge output for OAuth/CIMD activity
echo ""
echo "[4/5] Checking bridge logs for OAuth flow initiation..."

# Look for signs that mcp-remote attempted OAuth discovery or fetched CIMD
OAUTH_ATTEMPTED=0
if grep -qiE "(oauth|authorize|token|cimd|client.metadata|metadata.*fetch|fetch.*metadata)" "$BRIDGE_LOG" 2>/dev/null; then
    OAUTH_ATTEMPTED=1
    echo "  FOUND: Bridge attempted OAuth/CIMD activity"
    grep -iE "(oauth|authorize|token|cimd|client.metadata|metadata.*fetch|fetch.*metadata)" "$BRIDGE_LOG" | head -5 | sed 's/^/    /'
fi

# Check for HTTP errors from the stub's MCP endpoint (expected)
MCP_ERR_FOUND=0
if grep -qiE "(404|not found|unsupported|content.type|unexpected|fetch.*fail)" "$BRIDGE_LOG" 2>/dev/null; then
    MCP_ERR_FOUND=1
    echo "  FOUND: Bridge hit expected MCP endpoint error"
    grep -iE "(404|not found|unsupported|content.type|unexpected|fetch.*fail)" "$BRIDGE_LOG" | head -3 | sed 's/^/    /'
fi

# Step 5: Verify the bridge's client-metadata and callback configuration
echo ""
echo "[5/5] Verifying client metadata and callback configuration..."

# The CIMD URL is used as the client_id (SEP-991). mcp-remote does not
# download it; the AS does. So the local assertion is that mcp-remote
# (1) adopted the CIMD URL as the client identifier and
# (2) is using the 127.0.0.1 callback host the CIMD registered.
#
# The stub's own /cimd.json request count is reported for transparency only.
# It is NOT an assertion: mcp-remote never fetches the document locally, and
# the harness's own probe would otherwise make the count look meaningful.
CIMD_FETCHES=$(grep -c "200 /cimd.json" "$STUB_LOG" 2>/dev/null || echo 0)
echo "  stub /cimd.json 200s: $CIMD_FETCHES (harness probe only; mcp-remote does not fetch it)"

# (1) the client_id must be the CIMD URL, not a dynamically registered client
if grep -qi "Identifying this client by its metadata document" "$BRIDGE_LOG" 2>/dev/null; then
    echo "  CONFIRMED: client_id is the client metadata document (SEP-991)"
    grep -i "Identifying this client by its metadata document" "$BRIDGE_LOG" | head -2 | sed 's/^/    /'
elif grep -qi "registering instead" "$BRIDGE_LOG" 2>/dev/null; then
    echo "  WARNING: the AS does not accept a client metadata document, so"
    echo "           mcp-remote fell back to DYNAMIC REGISTRATION -- which"
    echo "           uses the default callback host (localhost) and breaks"
    echo "           the registered redirect_uri. The real Spotify Portal is"
    echo "           required to observe this branch."
    grep -i "registering instead" "$BRIDGE_LOG" | head -2 | sed 's/^/    /'
else
    echo "  NOTE: the local stub has no real AS, so the client-id branch was not"
    echo "        reached in this run. This is expected: the SEP-991 client-id"
    echo "        decision requires the AS's client_id_metadata_document_supported"
    echo "        flag, which only the production AS can provide."
fi

# (2) the callback host must be 127.0.0.1, matching the registered redirect_uri
CALLBACK_HOST_OK=0
if grep -qi "Using callback hostname: 127.0.0.1" "$BRIDGE_LOG" 2>/dev/null; then
    CALLBACK_HOST_OK=1
    echo "  CONFIRMED: callback hostname is 127.0.0.1 (matches the registered redirect_uri)"
    grep -i "Using callback hostname" "$BRIDGE_LOG" | head -2 | sed 's/^/    /'
else
    echo "  FAIL: callback hostname was not 127.0.0.1"
    grep -i "Using callback hostname" "$BRIDGE_LOG" | head -2 | sed 's/^/    /' || true
fi

# (3) mcp-remote must have derived a callback port and started the callback
CALLBACK_PORT_OK=0
if grep -qi "Using callback port derived from the server URL" "$BRIDGE_LOG" 2>/dev/null; then
    CALLBACK_PORT_OK=1
    echo "  CONFIRMED: callback port derived and callback server started"
    grep -i "Using callback port derived" "$BRIDGE_LOG" | head -1 | sed 's/^/    /'
fi

echo ""
echo "=== Results ==="
echo "  CIMD stub served over HTTPS : YES"
echo "  Bridge started              : YES"
echo "  OAuth flow attempt          : $(if [[ $OAUTH_ATTEMPTED -eq 1 ]]; then echo 'YES'; else echo 'NO'; fi)"
echo "  CIMD URL accepted           : $(if [[ $CIMD_ACCEPTED -eq 1 ]]; then echo 'YES'; else echo 'NO'; fi)"
echo "  Callback host = 127.0.0.1   : $(if [[ $CALLBACK_HOST_OK -eq 1 ]]; then echo 'YES'; else echo 'NO'; fi)"
echo "  Callback port derived       : $(if [[ $CALLBACK_PORT_OK -eq 1 ]]; then echo 'YES'; else echo 'NO'; fi)"

if [[ $CIMD_ACCEPTED -eq 1 && $CALLBACK_HOST_OK -eq 1 && $CALLBACK_PORT_OK -eq 1 ]]; then
    echo ""
    echo "PASS: mcp-remote accepted the client metadata document over HTTPS and"
    echo "      is using the 127.0.0.1 callback host that the CIMD registers."
    echo "      (Completing the OAuth flow requires the real Spotify Portal —"
    echo "      out of scope for this local test.)"
    TEST_FAILED=0
    exit 0
fi

echo ""
echo "FAIL: the bridge did not prove the client-metadata and callback"
echo "      configuration. Treat this as a regression, not a partial pass."
echo ""
echo "Debug: bridge log tail:"
tail -20 "$BRIDGE_LOG" 2>/dev/null | sed 's/^/  /'
exit 2