#!/usr/bin/env bash
# test-portal-bridge-live.sh — XJ-03 LIVE verification.
#
# Copy of test-portal-bridge.sh adapted to the PRODUCTION CIMD at
# https://auth.kooshapari.com/oauth/client-metadata.json and the live
# Spotify Portal MCP endpoint.
#
# This is an ADDITIVE verification script. It does not modify the
# master-stub test or scripts/portal-mcp-bridge.json.
#
# Exit codes:
#   0 = bridge reached the OAuth authorize step (PASS)
#   2 = bridge failed before/at the CIMD fetch (FAIL)
set -uo pipefail

CIMD_URL="https://auth.kooshapari.com/oauth/client-metadata.json"
MCP_URL="https://phenotype.spotifyportal.com/api/mcp-actions/v1"
WORK_DIR="${JCODE_SCRATCH_DIR:-/tmp}/xj03-live-$$"
mkdir -p "$WORK_DIR"
BRIDGE_LOG="$WORK_DIR/bridge.stderr.log"
INIT_LOG="$WORK_DIR/bridge.stdout.log"

echo "=== XJ-03 Portal Bridge LIVE verification ==="
echo "  CIMD URL : $CIMD_URL"
echo "  MCP URL  : $MCP_URL"
echo "  Workdir  : $WORK_DIR"

echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"xj03-live-harness","version":"0.1.0"}}}' > "$WORK_DIR/init-request.json"

# NOTE: mcp-remote requires the server URL as the FIRST positional arg.
# Putting --transport before the URL makes it parse '--transport' as the URL
# (TypeError: Invalid URL). The master portal-mcp-bridge.json has this bug.
timeout 90 bash -c "
    cat '$WORK_DIR/init-request.json' | \
    BROWSER=/usr/bin/true npx -y mcp-remote@0.15.1 \
        '$MCP_URL' \
        --transport http-only \
        --client-metadata-url '$CIMD_URL' \
        --resource '$MCP_URL' \
        --debug \
        2>'$BRIDGE_LOG' > '$INIT_LOG'
"
RC=$?
echo "  mcp-remote exit code: $RC"

echo "--- bridge stderr (tail) ---"
tail -30 "$BRIDGE_LOG" 2>/dev/null
echo "--- bridge stdout (tail) ---"
tail -10 "$INIT_LOG" 2>/dev/null

if grep -qiE "accounts\.spotify\.com/authorize|authorize\?" "$BRIDGE_LOG" "$INIT_LOG" 2>/dev/null; then
    echo "RESULT: PASS — bridge reached the OAuth authorize step"
    exit 0
fi
echo "RESULT: FAIL — bridge did not reach an authorize URL"
exit 2
