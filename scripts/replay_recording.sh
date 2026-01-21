#!/bin/bash
# Replay a jcode recording as video
#
# This script:
# 1. Starts a fresh jcode instance in a new terminal
# 2. Records the screen with wf-recorder
# 3. Replays the recorded keystrokes with proper timing
# 4. Outputs a video file
#
# Usage: ./replay_recording.sh <recording.json> [output.mp4]

set -e

RECORDING_FILE="${1:?Usage: $0 <recording.json> [output.mp4]}"
OUTPUT_FILE="${2:-${RECORDING_FILE%.json}.mp4}"

SCRIPT_DIR="$(dirname "$0")"

if [ ! -f "$RECORDING_FILE" ]; then
    echo "Error: Recording file not found: $RECORDING_FILE"
    exit 1
fi

echo "🎬 jcode Recording Replay"
echo "   Input:  $RECORDING_FILE"
echo "   Output: $OUTPUT_FILE"
echo ""

# Parse the recording and generate wtype commands
generate_wtype_script() {
    python3 << 'PYTHON' "$RECORDING_FILE"
import json
import sys

with open(sys.argv[1]) as f:
    events = json.load(f)

prev_time = 0
for event in events:
    offset = event.get('offset_ms', 0)
    delay = offset - prev_time
    prev_time = offset

    if delay > 0:
        # Sleep for the delay (in seconds)
        print(f"sleep {delay / 1000:.3f}")

    evt = event.get('event', {})
    evt_type = evt.get('type')

    if evt_type == 'Key':
        data = evt.get('data', {})
        code = data.get('code', '')
        mods = data.get('modifiers', [])

        # Convert code to wtype format
        key = None
        if code.startswith('Char('):
            # Extract character: Char('a') -> a
            char = code[6:-2] if len(code) > 7 else code[6:-1]
            key = char
        elif code == 'Enter':
            key = 'Return'
        elif code == 'Backspace':
            key = 'BackSpace'
        elif code == 'Tab':
            key = 'Tab'
        elif code == 'Esc':
            key = 'Escape'
        elif code == 'Up':
            key = 'Up'
        elif code == 'Down':
            key = 'Down'
        elif code == 'Left':
            key = 'Left'
        elif code == 'Right':
            key = 'Right'
        elif code == 'Home':
            key = 'Home'
        elif code == 'End':
            key = 'End'
        elif code == 'PageUp':
            key = 'Page_Up'
        elif code == 'PageDown':
            key = 'Page_Down'
        elif code == 'Delete':
            key = 'Delete'
        elif code == 'Insert':
            key = 'Insert'
        elif code.startswith('F') and code[1:].isdigit():
            key = code  # F1, F2, etc.
        else:
            # Unknown key, skip
            continue

        if key:
            cmd = 'wtype'
            for mod in mods:
                if mod == 'ctrl':
                    cmd += ' -M ctrl'
                elif mod == 'alt':
                    cmd += ' -M alt'
                elif mod == 'shift':
                    cmd += ' -M shift'

            if len(key) == 1 and key.isalnum():
                cmd += f' "{key}"'
            else:
                cmd += f' -k {key}'

            # Release modifiers
            for mod in reversed(mods):
                if mod == 'ctrl':
                    cmd += ' -m ctrl'
                elif mod == 'alt':
                    cmd += ' -m alt'
                elif mod == 'shift':
                    cmd += ' -m shift'

            print(cmd)
PYTHON
}

# Get the screen geometry for recording
GEOMETRY=$(niri msg focused-output 2>/dev/null | grep -oP 'Mode: \K\d+x\d+' | head -1 || echo "1920x1080")

echo "📹 Starting screen recording..."
echo "   Geometry: $GEOMETRY"
echo ""

# Start wf-recorder in background
wf-recorder -g "0,0 $GEOMETRY" -f "$OUTPUT_FILE" &
RECORDER_PID=$!
sleep 1  # Let recorder initialize

# Start jcode in a new kitty window
echo "🚀 Starting jcode..."
kitty --title "jcode-replay" -e bash -c "cd $(pwd) && ~/.cargo/bin/jcode; read -p 'Press Enter to close...'" &
KITTY_PID=$!
sleep 2  # Wait for jcode to start

# Focus the new window
sleep 0.5
# Find and focus the jcode-replay window
WINDOW_ID=$(niri msg windows 2>/dev/null | grep -B5 "jcode-replay" | grep -oP 'Window ID \K\d+' | head -1)
if [ -n "$WINDOW_ID" ]; then
    echo "   Focusing window $WINDOW_ID"
    niri msg action focus-window --id "$WINDOW_ID"
    sleep 0.3
fi

echo "⌨️  Replaying keystrokes..."
echo ""

# Generate and execute wtype script
generate_wtype_script | while read -r cmd; do
    if [[ "$cmd" == sleep* ]]; then
        eval "$cmd"
    else
        eval "$cmd" 2>/dev/null || true
    fi
done

echo ""
echo "⏹️  Stopping recording..."
sleep 1  # Final pause

# Stop recorder
kill $RECORDER_PID 2>/dev/null || true
wait $RECORDER_PID 2>/dev/null || true

# Clean up kitty window
kill $KITTY_PID 2>/dev/null || true

echo ""
echo "✅ Done!"
echo "   Video saved to: $OUTPUT_FILE"
ls -lh "$OUTPUT_FILE"
