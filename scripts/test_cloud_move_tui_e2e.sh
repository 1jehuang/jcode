#!/usr/bin/env bash
# Compatibility entry point. The maintained real-SSH TUI acceptance matrix is
# shared with the full CLI/agent/credential gate, including isolated artifacts.
set -euo pipefail
: "${JCODE_BIN:?Set JCODE_BIN to the jcode binary under test}"
exec python3 "$(dirname "$0")/test_cloud_acceptance.py" \
  --binary "$JCODE_BIN" --layers tui "$@"
