#!/bin/bash
# Prove that clicking a jcode notification focuses the terminal, not Script Editor.
#
# This exists because manual checks are unreliable: banners auto-dismiss, and
# "AppleScript can activate Ghostty" does not prove "clicking the banner does".
# So this script performs the click itself and asserts on observable state:
#
#   1. Park focus on a decoy app (Finder) so any focus change is meaningful.
#   2. Post the notification through the Jcode Notifier bundle.
#   3. Click the banner via Accessibility (no human timing involved).
#   4. Assert the frontmost app became the target terminal, and that
#      Script Editor never became frontmost.
#
# Exit 0 means the click is genuinely wired to the terminal.
set -uo pipefail

APP_DIR="${JCODE_NOTIFIER_APP_DIR:-$HOME/Applications/Jcode Notifier.app}"
HELPER="$APP_DIR/Contents/MacOS/jcode-notifier"
TARGET_BUNDLE_ID="${1:-${__CFBundleIdentifier:-com.apple.Terminal}}"
PROBE_FILE="$(mktemp "${TMPDIR:-/tmp}/jcode-notify-probe.XXXXXX")"
DECOY_APP="Finder"

fail() { echo "FAIL: $*" >&2; exit 1; }

[[ -x "$HELPER" ]] || fail "helper missing at $HELPER (run scripts/build_macos_notifier.sh)"

frontmost() {
  osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true' 2>/dev/null
}

target_name() {
  osascript -e "tell application id \"$TARGET_BUNDLE_ID\" to get name" 2>/dev/null \
    || echo "$TARGET_BUNDLE_ID"
}

echo "target bundle: $TARGET_BUNDLE_ID"

# 1. Park focus on the decoy so a later change to the terminal is unambiguous.
osascript -e "tell application \"$DECOY_APP\" to activate" >/dev/null 2>&1
sleep 1
BEFORE="$(frontmost)"
echo "frontmost before: $BEFORE"
[[ "$BEFORE" == "$DECOY_APP" ]] || echo "warn: decoy not frontmost (got $BEFORE)"

# 2. Post via the bundle. It stays resident so the click lands in-process.
pkill -f "jcode-notifier --post" >/dev/null 2>&1
open -a "$APP_DIR" --args --post \
  --title "jcode: validation" \
  --subtitle "automated click test" \
  --body "This banner is clicked automatically." \
  --sound Glass \
  --target-bundle-id "$TARGET_BUNDLE_ID" \
  --probe-file "$PROBE_FILE" || fail "could not launch helper"
sleep 3

# 3. Click the banner through Accessibility.
# macOS bash 3.2 mis-parses a heredoc inside $( ), so the AppleScript lives in
# a temp file instead.
CLICK_SCRIPT="$(mktemp "${TMPDIR:-/tmp}/jcode-notify-click.XXXXXX")"
cat > "$CLICK_SCRIPT" <<'APPLESCRIPT'
tell application "System Events"
  tell application process "NotificationCenter"
    if not (exists window 1) then return "no-banner"
    try
      -- Click the banner action area, which is what a user does.
      click window 1
      return "clicked-window"
    on error
      try
        click (first UI element of window 1 whose subrole is "AXNotificationCenterAlert")
        return "clicked-alert"
      on error errText
        return "click-error: " & errText
      end try
    end try
  end tell
end tell
APPLESCRIPT
CLICK_RESULT="$(osascript "$CLICK_SCRIPT" 2>&1)"
rm -f "$CLICK_SCRIPT"
echo "click result: $CLICK_RESULT"

# 4. Assert on observed state: did focus land on the terminal, and did Script
#    Editor ever steal it?
SAW_SCRIPT_EDITOR=0
FINAL=""
for _ in $(seq 1 10); do
  CURRENT="$(frontmost)"
  case "$CURRENT" in
    Script*|*"Script Editor"*) SAW_SCRIPT_EDITOR=1 ;;
  esac
  FINAL="$CURRENT"
  sleep 1
done

CLICK_LOGGED="$(cat "$PROBE_FILE" 2>/dev/null)"
rm -f "$PROBE_FILE"
pkill -f "jcode-notifier --post" >/dev/null 2>&1

echo "frontmost after: $FINAL"
echo "helper click record: ${CLICK_LOGGED:-<none>}"

case "$CLICK_RESULT" in
  no-banner) fail "no banner appeared: notifications are likely not permitted for Jcode (System Settings > Notifications > Jcode)" ;;
  click-error*) fail "could not click banner ($CLICK_RESULT); grant Accessibility control to your terminal in System Settings > Privacy & Security > Accessibility" ;;
esac

[[ -n "$CLICK_LOGGED" ]] || fail "helper never received the click, so the banner is not owned by the Jcode bundle"
[[ "$SAW_SCRIPT_EDITOR" -eq 0 ]] || fail "Script Editor became frontmost: notification is still attributed to osascript"

EXPECTED="$(target_name)"
if [[ "$FINAL" == "$EXPECTED" || "$FINAL" == *"$EXPECTED"* ]]; then
  echo "PASS: clicking the notification focused '$FINAL' (target $TARGET_BUNDLE_ID), Script Editor never appeared"
  exit 0
fi
fail "expected frontmost '$EXPECTED' after click, got '$FINAL'"
