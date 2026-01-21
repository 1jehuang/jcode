#!/bin/bash
# Screenshot Watcher - monitors for jcode screenshot signals
#
# This script watches the signal directory and captures screenshots
# when jcode signals that a specific UI state is ready.
#
# Usage: ./screenshot_watcher.sh [window_id]
#
# Start jcode with: /screenshot-mode on

set -e

SIGNAL_DIR="${XDG_RUNTIME_DIR:-/tmp}/jcode-screenshots"
OUTPUT_DIR="$(dirname "$0")/../docs/screenshots"
WINDOW_ID="${1:-}"

mkdir -p "$OUTPUT_DIR" "$SIGNAL_DIR"

# Get window ID if not provided
if [ -z "$WINDOW_ID" ]; then
    echo "Tip: Pass window ID as argument, or we'll use focused window for each capture"
fi

echo "🎬 Screenshot Watcher"
echo "   Signal dir: $SIGNAL_DIR"
echo "   Output dir: $OUTPUT_DIR"
echo "   Window ID: ${WINDOW_ID:-<focused>}"
echo ""
echo "Waiting for signals... (Ctrl+C to stop)"
echo "Enable in jcode with: /screenshot-mode on"
echo ""

capture_signal() {
    local file="$1"
    local state_name="${file%.ready}"
    local signal_path="$SIGNAL_DIR/$file"
    local output_path="$OUTPUT_DIR/${state_name}.png"

    echo "📸 Signal: $state_name"

    # Read metadata from signal file
    if [ -f "$signal_path" ]; then
        cat "$signal_path" | jq . 2>/dev/null || true
    fi

    # Small delay to ensure UI is fully rendered
    sleep 0.15

    # Focus window if ID provided, otherwise use current focus
    if [ -n "$WINDOW_ID" ]; then
        niri msg action focus-window --id "$WINDOW_ID"
        sleep 0.1
    fi

    # Capture screenshot
    niri msg action screenshot-window --path "$output_path"

    # Clear the signal
    rm -f "$signal_path"

    echo "   ✅ Saved: $output_path"
    echo ""
}

# Try inotifywait first, fall back to polling
if command -v inotifywait &>/dev/null; then
    echo "Using inotifywait for efficient watching..."
    inotifywait -m -e create -e modify "$SIGNAL_DIR" 2>/dev/null | while read -r dir event file; do
        if [[ "$file" == *.ready ]]; then
            capture_signal "$file"
        fi
    done
else
    echo "Using polling (install inotify-tools for better performance)..."
    SEEN_FILES=""
    while true; do
        for file in "$SIGNAL_DIR"/*.ready 2>/dev/null; do
            [ -e "$file" ] || continue
            basename_file=$(basename "$file")
            if [[ ! " $SEEN_FILES " =~ " $basename_file " ]]; then
                capture_signal "$basename_file"
                SEEN_FILES="$SEEN_FILES $basename_file"
            fi
        done
        sleep 0.2
    done
fi
