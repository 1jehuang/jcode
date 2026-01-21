#!/bin/bash
# Capture jcode screenshots with your actual terminal theme
# Usage: ./capture_screenshot.sh [output_name]

set -e

OUTPUT_DIR="$(dirname "$0")/../docs/screenshots"
OUTPUT_NAME="${1:-jcode-screenshot}"
OUTPUT_PATH="$OUTPUT_DIR/${OUTPUT_NAME}.png"

mkdir -p "$OUTPUT_DIR"

echo "📸 jcode Screenshot Capture"
echo ""
echo "Instructions:"
echo "  1. Make sure jcode is running in a visible terminal"
echo "  2. Set up the UI state you want to capture"
echo "  3. Press Enter here, then click on the jcode window"
echo ""
read -p "Press Enter when ready..."

# Use slurp to let user select a window/region, then capture with grim
GEOMETRY=$(slurp)
if [ -n "$GEOMETRY" ]; then
    grim -g "$GEOMETRY" "$OUTPUT_PATH"
    echo "✅ Saved to: $OUTPUT_PATH"

    # Show the image dimensions
    if command -v file &>/dev/null; then
        file "$OUTPUT_PATH"
    fi
else
    echo "❌ No region selected"
    exit 1
fi
