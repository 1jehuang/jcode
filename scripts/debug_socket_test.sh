#!/bin/bash
# Test script to capture and analyze debug socket events
# Usage: ./scripts/debug_socket_test.sh [capture|compare]

DEBUG_SOCKET="${XDG_RUNTIME_DIR:-/tmp}/jcode-debug.sock"
CAPTURE_FILE="/tmp/jcode_debug_capture.jsonl"

case "${1:-capture}" in
    capture)
        echo "Connecting to debug socket: $DEBUG_SOCKET"
        echo "Saving events to: $CAPTURE_FILE"
        echo "Press Ctrl+C to stop"
        echo "---"
        nc -U "$DEBUG_SOCKET" | tee "$CAPTURE_FILE" | jq -c '.'
        ;;

    snapshot)
        echo "Getting state snapshot from debug socket..."
        # Connect and get just the first message (snapshot)
        timeout 1 nc -U "$DEBUG_SOCKET" | head -1 | jq '.'
        ;;

    watch)
        echo "Watching debug socket events (pretty print)..."
        nc -U "$DEBUG_SOCKET" | jq '.'
        ;;

    analyze)
        if [ -f "$CAPTURE_FILE" ]; then
            echo "Analyzing captured events..."
            echo ""
            echo "Event types:"
            jq -r '.type' "$CAPTURE_FILE" | sort | uniq -c | sort -rn
            echo ""
            echo "Total events: $(wc -l < "$CAPTURE_FILE")"
        else
            echo "No capture file found. Run 'capture' first."
        fi
        ;;

    *)
        echo "Usage: $0 [capture|snapshot|watch|analyze]"
        echo "  capture  - Capture events to file and display"
        echo "  snapshot - Get initial state snapshot"
        echo "  watch    - Watch events in real-time (pretty)"
        echo "  analyze  - Analyze captured events"
        ;;
esac
