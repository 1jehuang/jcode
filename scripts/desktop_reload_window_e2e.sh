#!/usr/bin/env bash
set -euo pipefail

# End-to-end smoke test for desktop stable-host /reload behavior under niri.
#
# It launches jcode-desktop in stable-host mode, records the compositor window id
# and layout, injects `/reload`, then verifies the same OS window is still present
# with the same niri placement and the app-worker child process changed. This catches
# regressions where slash reload falls back to the old full-process handoff path
# that closes/reopens the desktop window.
#
# Requirements: niri, jq, wtype, a Wayland session, and a built jcode-desktop.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${JCODE_DESKTOP_BIN:-$ROOT_DIR/target/debug/jcode-desktop}"
TIMEOUT_SECS="${JCODE_DESKTOP_RELOAD_E2E_TIMEOUT_SECS:-15}"
LOG_FILE="${JCODE_DESKTOP_RELOAD_E2E_LOG:-$(mktemp -t jcode-desktop-reload-e2e.XXXXXX.log)}"

if [[ ! -x "$BIN" ]]; then
  echo "desktop binary not found or not executable: $BIN" >&2
  echo "hint: cargo build -p jcode-desktop --bin jcode-desktop" >&2
  exit 2
fi
for tool in niri jq wtype; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "required tool not found: $tool" >&2
    exit 2
  fi
done

cleanup() {
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

child_pids() {
  pgrep -P "$APP_PID" 2>/dev/null | sort | tr '\n' ' ' | sed 's/[[:space:]]*$//'
}

layouts_match_with_tolerance() {
  local before="$1"
  local after="$2"
  jq -e -n --argjson before "$before" --argjson after "$after" '
    def abs: if . < 0 then -. else . end;
    def close($a; $b): (($a - $b) | abs) <= 2;
    ($before.id == $after.id)
      and ($before.pid == $after.pid)
      and ($before.workspace_id == $after.workspace_id)
      and ($before.is_floating == $after.is_floating)
      and ($before.layout.pos_in_scrolling_layout == $after.layout.pos_in_scrolling_layout)
      and ($before.layout.tile_pos_in_workspace_view == $after.layout.tile_pos_in_workspace_view)
      and ($before.layout.window_offset_in_tile == $after.layout.window_offset_in_tile)
      and close($before.layout.tile_size[0]; $after.layout.tile_size[0])
      and close($before.layout.tile_size[1]; $after.layout.tile_size[1])
      and close($before.layout.window_size[0]; $after.layout.window_size[0])
      and close($before.layout.window_size[1]; $after.layout.window_size[1])
  ' >/dev/null
}

"$BIN" \
  --desktop-process-role stable-host \
  --startup-log \
  >"$LOG_FILE" 2>&1 &
APP_PID=$!

deadline=$((SECONDS + TIMEOUT_SECS))
WINDOW_JSON=""
while (( SECONDS < deadline )); do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "desktop process exited before window appeared" >&2
    cat "$LOG_FILE" >&2 || true
    exit 1
  fi
  WINDOW_JSON="$(niri msg -j windows | jq -c --argjson pid "$APP_PID" 'map(select(.pid == $pid)) | first // empty')"
  if [[ -n "$WINDOW_JSON" ]]; then
    break
  fi
  sleep 0.1
done

if [[ -z "$WINDOW_JSON" ]]; then
  echo "timed out waiting for desktop window for pid $APP_PID" >&2
  cat "$LOG_FILE" >&2 || true
  exit 1
fi

WINDOW_ID="$(jq -r '.id' <<<"$WINDOW_JSON")"
BEFORE_LAYOUT="$(jq -c '{id, pid, workspace_id, is_floating, layout}' <<<"$WINDOW_JSON")"
BEFORE_CHILDREN=""
deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  BEFORE_CHILDREN="$(child_pids)"
  if [[ -n "$BEFORE_CHILDREN" ]]; then
    break
  fi
  sleep 0.1
done
if [[ -z "$BEFORE_CHILDREN" ]]; then
  echo "timed out waiting for app-worker child process" >&2
  cat "$LOG_FILE" >&2 || true
  exit 1
fi

niri msg action focus-window --id "$WINDOW_ID" >/dev/null
sleep 0.2
# Trigger the user-visible slash command path. In stable-host mode this should
# request a host-side app-worker restart, not a full desktop process handoff.
wtype '/reload'
wtype -k Return

# Wait for the reload request to be processed. The stable-host path should keep
# the same compositor window alive while restarting only the app worker.
deadline=$((SECONDS + TIMEOUT_SECS))
AFTER_CHILDREN=""
while (( SECONDS < deadline )); do
  AFTER_CHILDREN="$(child_pids)"
  if [[ -n "$AFTER_CHILDREN" && "$AFTER_CHILDREN" != "$BEFORE_CHILDREN" ]]; then
    break
  fi
  sleep 0.1
done
if [[ -z "$AFTER_CHILDREN" || "$AFTER_CHILDREN" == "$BEFORE_CHILDREN" ]]; then
  echo "app-worker child process did not change after hot reload trigger" >&2
  echo "before children: $BEFORE_CHILDREN" >&2
  echo "after children:  $AFTER_CHILDREN" >&2
  cat "$LOG_FILE" >&2 || true
  exit 1
fi

AFTER_JSON="$(niri msg -j windows | jq -c --argjson id "$WINDOW_ID" 'map(select(.id == $id)) | first // empty')"
if [[ -z "$AFTER_JSON" ]]; then
  echo "window id $WINDOW_ID disappeared after /reload" >&2
  echo "before: $BEFORE_LAYOUT" >&2
  cat "$LOG_FILE" >&2 || true
  exit 1
fi

AFTER_LAYOUT="$(jq -c '{id, pid, workspace_id, is_floating, layout}' <<<"$AFTER_JSON")"
if ! layouts_match_with_tolerance "$BEFORE_LAYOUT" "$AFTER_LAYOUT"; then
  echo "window layout changed after /reload" >&2
  echo "before: $BEFORE_LAYOUT" >&2
  echo "after:  $AFTER_LAYOUT" >&2
  cat "$LOG_FILE" >&2 || true
  exit 1
fi

echo "desktop reload window e2e ok: window_id=$WINDOW_ID host_pid=$APP_PID before_worker='$BEFORE_CHILDREN' after_worker='$AFTER_CHILDREN' log=$LOG_FILE"
