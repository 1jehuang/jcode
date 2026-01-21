#!/bin/bash
# Autonomous screenshot capture for jcode documentation
# Uses niri window management + screenshot capabilities
#
# Usage: ./auto_screenshot.sh <window_id> <output_name> [setup_command]
#
# Examples:
#   ./auto_screenshot.sh 77 main-ui
#   ./auto_screenshot.sh 77 info-widget "/info"
#   ./auto_screenshot.sh 77 command-palette "/"

set -e

WINDOW_ID="${1:?Usage: $0 <window_id> <output_name> [setup_command]}"
OUTPUT_NAME="${2:?Usage: $0 <window_id> <output_name> [setup_command]}"
SETUP_CMD="${3:-}"

OUTPUT_DIR="$(dirname "$0")/../docs/screenshots"
OUTPUT_PATH="$OUTPUT_DIR/${OUTPUT_NAME}.png"

mkdir -p "$OUTPUT_DIR"

echo "📸 Capturing window $WINDOW_ID as $OUTPUT_NAME"

# Focus the target window
niri msg action focus-window --id "$WINDOW_ID"
sleep 0.3  # Let the window focus settle

# If there's a setup command, we'd need to inject it somehow
# For now, this is a placeholder - see below for the full solution
if [ -n "$SETUP_CMD" ]; then
    echo "⚠️  Setup command '$SETUP_CMD' - manual injection needed for now"
    echo "   Press Enter after setting up the UI state..."
    read -r
fi

# Screenshot the focused window
niri msg action screenshot-window --path "$OUTPUT_PATH"

echo "✅ Saved: $OUTPUT_PATH"
ls -lh "$OUTPUT_PATH"
