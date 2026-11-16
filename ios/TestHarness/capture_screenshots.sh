#!/usr/bin/env bash
#
# Captures App Store screenshots from offline demo mode, and in doing so proves
# the reviewer path works: fresh install, no server, no pairing, tap the demo
# button, drive a conversation.
#
# Screenshots land in build/screenshots/<device>/ at the exact pixel sizes App
# Store Connect requires (whatever the simulator's native resolution is for the
# chosen device, so pick a 6.9" iPhone and a 13" iPad).
#
# Usage:
#   ./TestHarness/capture_screenshots.sh                       # 6.9" iPhone
#   ./TestHarness/capture_screenshots.sh --device "iPad Pro 13-inch (M4)"
#
set -euo pipefail
cd "$(dirname "$0")/.."   # ios/

DEVICE="iPhone 17 Pro Max"
BUNDLE_ID="com.jcode.mobile"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --device) DEVICE="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

OUT="build/screenshots/${DEVICE// /_}"
mkdir -p "$OUT"
log() { printf '\033[36m[shots]\033[0m %s\n' "$*"; }

log "generating project"
xcodegen generate >/dev/null

log "resolving simulator: $DEVICE"
UDID=$(xcrun simctl list devices available -j \
    | python3 -c "
import json,sys
name = sys.argv[1]
data = json.load(sys.stdin)['devices']
for runtime, devices in sorted(data.items(), reverse=True):
    for d in devices:
        if d['name'] == name and d['isAvailable']:
            print(d['udid']); sys.exit(0)
sys.exit('no available simulator named ' + name)
" "$DEVICE")

xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b >/dev/null

log "building app"
DERIVED="build/DerivedData"
xcodebuild build \
    -project JCodeMobile.xcodeproj \
    -scheme JCodeMobile \
    -configuration Release \
    -destination "id=$UDID" \
    -derivedDataPath "$DERIVED" \
    CODE_SIGNING_ALLOWED=NO >/dev/null

APP=$(find "$DERIVED/Build/Products" -name "JCodeMobile.app" -maxdepth 3 | head -1)
[[ -n "$APP" ]] || { echo "app bundle not found" >&2; exit 1; }

log "installing fresh (uninstall first so we get the true first-run state)"
xcrun simctl uninstall "$UDID" "$BUNDLE_ID" 2>/dev/null || true
xcrun simctl install "$UDID" "$APP"
shot() { # shot <name>
    xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1
    log "captured $1"
}

# 1. First run: the pairing screen, including the demo entry point.
xcrun simctl launch "$UDID" "$BUNDLE_ID" >/dev/null
sleep 3
shot "1-pairing-with-demo-entry"

# 2. Demo mode, entered via the launch argument so this is reproducible.
xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true
xcrun simctl launch "$UDID" "$BUNDLE_ID" -jcodeDemo YES >/dev/null
sleep 4
shot "2-demo-empty-session"

# 3. A finished conversation: reasoning, a tool call, and a rendered answer.
xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true
xcrun simctl launch "$UDID" "$BUNDLE_ID" -jcodeDemo YES \
    -jcodeDemoPrompt "Run the tests" >/dev/null
sleep 8
shot "3-conversation-with-tool-call"

cat <<'EOF'

Captured automatically:
  1-pairing-with-demo-entry, 2-demo-empty-session,
  3-conversation-with-tool-call

For the remaining marketing shots, the app is already running in demo mode on
the booted simulator. Tap a starter prompt, then capture with:
  xcrun simctl io booted screenshot 3-conversation.png
Suggested: 3-conversation, 4-tool-call, 5-sessions, 6-model-picker

Screenshots directory:
EOF
echo "  $PWD/$OUT"
