#!/bin/bash
# Full autonomous demo capture for jcode
# Captures screenshots of various UI states using niri + wtype
#
# Usage: ./capture_demo.sh [window_id]
#   If window_id not provided, uses the focused window

set -e

SCRIPT_DIR="$(dirname "$0")"
OUTPUT_DIR="$SCRIPT_DIR/../docs/screenshots"
WINDOW_ID="${1:-}"

mkdir -p "$OUTPUT_DIR"

# Get window ID if not provided
if [ -z "$WINDOW_ID" ]; then
    WINDOW_ID=$(niri msg focused-window 2>&1 | head -1 | grep -oP 'Window ID \K\d+')
    echo "Using focused window: $WINDOW_ID"
fi

capture() {
    local name="$1"
    local keys="$2"
    local delay="${3:-0.5}"

    echo "📸 Capturing: $name"

    # Focus the window
    niri msg action focus-window --id "$WINDOW_ID"
    sleep 0.2

    # Inject keystrokes if provided
    if [ -n "$keys" ]; then
        echo "   Typing: $keys"
        wtype "$keys"
        sleep "$delay"
    fi

    # Screenshot
    niri msg action screenshot-window --path "$OUTPUT_DIR/${name}.png"
    echo "   ✅ Saved: $OUTPUT_DIR/${name}.png"
}

clear_input() {
    # Clear any existing input with Ctrl+U, then Escape to close popups
    wtype -M ctrl -k u
    sleep 0.1
    wtype -k Escape
    sleep 0.2
}

echo "🎬 jcode Demo Capture"
echo "   Window ID: $WINDOW_ID"
echo "   Output: $OUTPUT_DIR"
echo ""

# Capture sequence
niri msg action focus-window --id "$WINDOW_ID"
sleep 0.3

# 1. Main UI (clean state)
clear_input
capture "main-ui" "" 0.3

# 2. Command palette (type /)
clear_input
capture "command-palette" "/" 0.3

# 3. Close palette, show help
wtype -k Escape
sleep 0.2
capture "help-view" "/help" 0.5

# Clean up - close any popups
wtype -k Escape
sleep 0.2

echo ""
echo "🎉 Done! Screenshots saved to $OUTPUT_DIR/"
ls -la "$OUTPUT_DIR/"*.png 2>/dev/null || echo "No screenshots found"
