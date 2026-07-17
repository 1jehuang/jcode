#!/usr/bin/env bash
# Uninstall jcode binaries and (optionally) all user data.
#
# Default: removes installed binaries, build channels, and the launcher
# symlink, but keeps user data (config, auth, sessions, logs) so a clean
# reinstall picks up where you left off.
#
# Flags:
#   --purge     Also delete ~/.jcode (config, auth, sessions, logs, memory).
#   --dry-run   Print what would be removed without deleting anything.
#   --yes       Skip the confirmation prompt.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/uninstall.sh | bash
#   bash scripts/uninstall.sh --purge
set -euo pipefail

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m%s\033[0m\n' "$*"; }
err()  { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

PURGE=false
DRY_RUN=false
ASSUME_YES=false

for arg in "$@"; do
  case "$arg" in
    --purge)   PURGE=true ;;
    --dry-run) DRY_RUN=true ;;
    --yes|-y)  ASSUME_YES=true ;;
    --help|-h)
      sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) err "Unknown flag: $arg (supported: --purge, --dry-run, --yes)" ;;
  esac
done

OS="$(uname -s)"
case "$OS" in
  MINGW*|MSYS*|CYGWIN*)
    JCODE_HOME="${LOCALAPPDATA:?LOCALAPPDATA not set}/jcode"
    LAUNCHER_DIR="${JCODE_INSTALL_DIR:-$LOCALAPPDATA/jcode/bin}"
    LAUNCHER="$LAUNCHER_DIR/jcode.exe"
    BUILDS_DIR="$JCODE_HOME/builds"
    USER_DATA_DIR="$JCODE_HOME"
    ;;
  *)
    JCODE_HOME="$HOME/.jcode"
    LAUNCHER_DIR="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
    LAUNCHER="$LAUNCHER_DIR/jcode"
    BUILDS_DIR="$JCODE_HOME/builds"
    USER_DATA_DIR="$JCODE_HOME"
    ;;
esac

# Collect removal targets.
TARGETS=()
[ -e "$LAUNCHER" ] || [ -L "$LAUNCHER" ] && TARGETS+=("$LAUNCHER (launcher)")
[ -d "$BUILDS_DIR" ] && TARGETS+=("$BUILDS_DIR (installed binaries: stable/current/canary/versions)")
if [ "$PURGE" = true ] && [ -d "$USER_DATA_DIR" ]; then
  TARGETS+=("$USER_DATA_DIR (ALL user data: config, auth, sessions, logs, memory)")
fi

# Compatibility wrapper installed by some setups.
SELFDEV_WRAPPER="$HOME/.local/bin/selfdev"
if [ -f "$SELFDEV_WRAPPER" ] && grep -q "jcode" "$SELFDEV_WRAPPER" 2>/dev/null; then
  TARGETS+=("$SELFDEV_WRAPPER (selfdev wrapper)")
fi

if [ ${#TARGETS[@]} -eq 0 ]; then
  info "Nothing to uninstall: no jcode installation found."
  exit 0
fi

info "The following will be removed:"
for t in "${TARGETS[@]}"; do
  printf '  - %s\n' "$t"
done
if [ "$PURGE" = false ]; then
  warn "User data in $USER_DATA_DIR is kept (config, auth, sessions, logs)."
  warn "Run with --purge for a full wipe."
fi

if [ "$DRY_RUN" = true ]; then
  info "Dry run: nothing was deleted."
  exit 0
fi

if [ "$ASSUME_YES" = false ]; then
  if [ -t 0 ]; then
    printf 'Proceed? [y/N] '
    read -r reply
    case "$reply" in
      y|Y|yes|YES) ;;
      *) info "Aborted."; exit 1 ;;
    esac
  else
    # Piped (curl | bash): require explicit --yes to avoid accidental deletion.
    err "stdin is not a terminal; re-run with --yes to confirm (e.g. curl ... | bash -s -- --yes)"
  fi
fi

# Stop any running jcode server so files are not recreated mid-wipe.
if command -v pkill >/dev/null 2>&1; then
  pkill -f 'jcode( .*)? serve' 2>/dev/null || true
fi

remove() {
  local path="$1"
  if [ -e "$path" ] || [ -L "$path" ]; then
    rm -rf -- "$path"
    info "Removed $path"
  fi
}

remove "$LAUNCHER"
if [ "$PURGE" = true ]; then
  remove "$USER_DATA_DIR"
else
  remove "$BUILDS_DIR"
fi
if [ -f "$SELFDEV_WRAPPER" ] && grep -q "jcode" "$SELFDEV_WRAPPER" 2>/dev/null; then
  remove "$SELFDEV_WRAPPER"
fi

# On Windows (Git Bash), also remove the launcher dir from the persisted user
# PATH, mirroring what install.sh added. Case- and trailing-slash-insensitive,
# best-effort: a PATH cleanup failure must not fail the uninstall.
case "$OS" in
  MINGW*|MSYS*|CYGWIN*)
    if command -v powershell.exe >/dev/null 2>&1; then
      win_launcher_dir=$(cygpath -w "$LAUNCHER_DIR" 2>/dev/null || echo "$LAUNCHER_DIR")
      _win_path_key() { printf '%s' "$1" | sed 's|[\\/]*$||' | tr '[:upper:]' '[:lower:]'; }
      current_user_path=$(powershell.exe -NoProfile -NonInteractive -Command \
        "[Environment]::GetEnvironmentVariable('Path','User')" 2>/dev/null | tr -d '\r' || true)
      target_key=$(_win_path_key "$win_launcher_dir")
      new_user_path=""
      set -f
      IFS=';'
      for entry in $current_user_path; do
        [ -n "$entry" ] || continue
        [ "$(_win_path_key "$entry")" = "$target_key" ] && continue
        if [ -z "$new_user_path" ]; then new_user_path="$entry"; else new_user_path="$new_user_path;$entry"; fi
      done
      unset IFS
      set +f
      if [ "$new_user_path" != "$current_user_path" ]; then
        if JCODE_NEW_USER_PATH="$new_user_path" powershell.exe -NoProfile -NonInteractive -Command \
          '[Environment]::SetEnvironmentVariable("Path", $env:JCODE_NEW_USER_PATH, "User")' >/dev/null 2>&1; then
          info "Removed $win_launcher_dir from your user PATH."
        fi
      fi
    fi
    ;;
esac

info "jcode uninstalled."
if [ "$PURGE" = false ]; then
  info "Reinstall with: curl -fsSL https://jcode.sh/install | bash"
else
  info "All jcode data wiped. Reinstall with: curl -fsSL https://jcode.sh/install | bash"
fi
