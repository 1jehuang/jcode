#!/bin/bash
# jcode demo recording orchestrator
# Usage: ./scripts/record_demo.sh <demo_name> <prompt>
#
# This script:
# 1. Opens a fresh jcode in a new kitty window
# 2. Starts wf-recorder on that window
# 3. Sends the prompt to jcode
# 4. Waits for completion, then stops recording

set -euo pipefail

DEMO_NAME="${1:?Usage: record_demo.sh <name> <prompt>}"
PROMPT="${2:?Usage: record_demo.sh <name> <prompt>}"
DEMO_DIR="/tmp/jcode-demo/$DEMO_NAME"
OUTPUT_DIR="$HOME/jcode/assets/demos"
SOCK=$(ls /tmp/kitty.sock* 2>/dev/null | head -1)

mkdir -p "$DEMO_DIR" "$OUTPUT_DIR"

echo "=== jcode Demo Recorder ==="
echo "Demo: $DEMO_NAME"
echo "Prompt: $PROMPT"
echo "Working dir: $DEMO_DIR"
echo ""

# Step 1: Launch jcode in a new kitty OS window
echo "[1/5] Launching jcode..."
kitten @ --to unix:$SOCK launch --type=os-window \
    --cwd "$DEMO_DIR" \
    --title "jcode-demo-$DEMO_NAME" \
    /home/jeremy/jcode/target/release/jcode

sleep 3  # Let jcode fully start

# Step 2: Find the window
DEMO_WIN_ID=$(niri msg windows 2>/dev/null | grep -B5 "jcode-demo-$DEMO_NAME" | grep "Window ID" | awk '{print $3}' | tr -d ':')
if [ -z "$DEMO_WIN_ID" ]; then
    echo "ERROR: Could not find demo window"
    exit 1
fi
echo "[2/5] Found window ID: $DEMO_WIN_ID"

# Focus the window
niri msg action focus-window --id "$DEMO_WIN_ID"
sleep 0.5

# Step 3: Start recording
echo "[3/5] Starting recording..."
RECORDING_FILE="$OUTPUT_DIR/${DEMO_NAME}.mp4"
wf-recorder -f "$RECORDING_FILE" &
RECORDER_PID=$!
sleep 1

# Step 4: Type the prompt into jcode
echo "[4/5] Sending prompt..."
# Find the kitty window id
KITTY_WIN_ID=$(kitten @ --to unix:$SOCK ls 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
for os_win in data:
    for tab in os_win.get('tabs', []):
        for win in tab.get('windows', []):
            if 'jcode-demo-$DEMO_NAME' in win.get('title', ''):
                print(win['id'])
                sys.exit(0)
")

if [ -n "$KITTY_WIN_ID" ]; then
    # Type the prompt character by character with small delay for visual effect
    kitten @ --to unix:$SOCK send-text --match "id:$KITTY_WIN_ID" "$PROMPT"
    sleep 0.5
    # Press Enter
    kitten @ --to unix:$SOCK send-text --match "id:$KITTY_WIN_ID" $'\r'
else
    echo "WARNING: Could not find kitty window, trying by title match..."
    kitten @ --to unix:$SOCK send-text --match "title:jcode-demo-$DEMO_NAME" "$PROMPT"
    sleep 0.5
    kitten @ --to unix:$SOCK send-text --match "title:jcode-demo-$DEMO_NAME" $'\r'
fi

echo "Prompt sent. Waiting for completion..."
echo "(Press Ctrl+C to stop recording early, or wait for auto-detection)"

# Step 5: Wait and then stop recording
# Poll for completion - check if jcode is still processing
# Simple approach: wait for a fixed time or manual Ctrl+C
trap 'echo "Stopping..."; kill $RECORDER_PID 2>/dev/null; wait $RECORDER_PID 2>/dev/null; echo "Recording saved: $RECORDING_FILE"' INT

# Wait for the agent to finish (poll the debug socket)
MAX_WAIT=180  # 3 minutes max
ELAPSED=0
while [ $ELAPSED -lt $MAX_WAIT ]; do
    sleep 5
    ELAPSED=$((ELAPSED + 5))
    
    # Check if the kitty window title indicates idle (no streaming)
    CURRENT_TITLE=$(kitten @ --to unix:$SOCK ls 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
for os_win in data:
    for tab in os_win.get('tabs', []):
        for win in tab.get('windows', []):
            if win['id'] == $KITTY_WIN_ID:
                print(win.get('title', ''))
                sys.exit(0)
" 2>/dev/null || echo "unknown")
    
    echo "  [${ELAPSED}s] Window: $CURRENT_TITLE"
done

# Add a small pause at the end so viewer can see the result
sleep 3

# Stop recording
kill $RECORDER_PID 2>/dev/null
wait $RECORDER_PID 2>/dev/null

echo ""
echo "[5/5] Recording saved: $RECORDING_FILE"
FILE_SIZE=$(du -h "$RECORDING_FILE" | cut -f1)
echo "Size: $FILE_SIZE"
echo ""
echo "To convert to GIF: ffmpeg -i $RECORDING_FILE -vf 'fps=15,scale=800:-1' ${RECORDING_FILE%.mp4}.gif"
