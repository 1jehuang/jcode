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
#   0 = bridge reached the OAuth authorize step, or completed an
#       authenticated MCP initialize from a cached token (PASS)
#   2 = bridge neither reached an authorize URL nor completed an
#       initialize (FAIL)
#
# Two 2026-10-19 corrections:
#   1. BROWSER=/usr/bin/true is NOT set here. That silently swallowed the
#      consent page (the browser "opened" /usr/bin/true, exit 0, no tab).
#      The bridge then waited forever for a callback that could never arrive,
#      holding 127.0.0.1:47290 and leaving an orphan mcp-remote behind.
#   2. --host 127.0.0.1 is REQUIRED. The CIMD registers
#      redirect_uris=["http://127.0.0.1:47290/oauth/callback"], but
#      mcp-remote defaults to "localhost" on non-Windows. The AS rejects the
#      mismatch with HTTP 400 InputError "Invalid redirect_uri ... not
#      registered in client metadata". Use --host to match the CIMD.
#   3. MCP_REMOTE_CONFIG_DIR is pinned under WORK_DIR. Without it the run
#      reuses ~/.mcp-auth, which lets a cached token complete initialization
#      without ever printing an authorize URL. Isolating the config dir makes
#      every live check start from a fresh OAuth flow.
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
# --host 127.0.0.1 must match the redirect_uri registered in the CIMD.
timeout 90 bash -c "
    cat '$WORK_DIR/init-request.json' | \
    env -u BROWSER MCP_REMOTE_CONFIG_DIR='$WORK_DIR/mcp-auth' \
        npx -y mcp-remote@0.15.1 \
        '$MCP_URL' \
        --transport http-only \
        --client-metadata-url '$CIMD_URL' \
        --resource '$MCP_URL' \
        --host 127.0.0.1 \
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

# A warm token cache lets mcp-remote complete MCP initialization WITHOUT
# printing an authorize URL. That is a successful authenticated run, not a
# failure, so accept a real JSON-RPC initialize response as PASS too.
if grep -qE '"serverInfo"|"protocolVersion"' "$INIT_LOG" 2>/dev/null; then
    echo "RESULT: PASS — bridge completed an authenticated MCP initialize (cached token)"
    exit 0
fi

echo "RESULT: FAIL — bridge neither reached an authorize URL nor completed an initialize"
exit 2
