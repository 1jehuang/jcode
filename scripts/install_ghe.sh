#!/usr/bin/env bash
# Install an already-built GHE binary without changing the stock stable channel.
# Requires [features] check_updates = false in ~/.jcode/config.toml.
set -Eeuo pipefail
umask 077

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILDS_DIR="$HOME/.jcode/builds"
INSTALL_DIR="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
BACKUPS_DIR="$BUILDS_DIR/ghe-install-backups"
CONFIG_FILE="$HOME/.jcode/config.toml"
readonly REPO_ROOT BUILDS_DIR INSTALL_DIR BACKUPS_DIR CONFIG_FILE
PATHS=(
  "$BUILDS_DIR/current/jcode"
  "$BUILDS_DIR/shared-server/jcode"
  "$BUILDS_DIR/current-version"
  "$BUILDS_DIR/shared-server-version"
  "$INSTALL_DIR/jcode"
)
BACKUP_DIR=""
STAGING_DIR=""
PREVIOUS_SERVER=""
HAS_CHANGED_CHANNELS=0
HAS_ATTEMPTED_RELOAD=0

usage() {
  cat <<EOF
Usage: bash $0 [already-built-binary]
       bash $0 --rollback <backup-directory>

Default binary: $REPO_ROOT/target/selfdev/jcode
Installs a SHA-256-addressed immutable custom-ghe version, promotes current
and shared-server, and gracefully reloads the shared server with session handoff.
Stable and all existing immutable versions remain untouched. No Cargo commands,
config edits, shell startup edits, credentials, or provider requests are made.
Requires [features] check_updates = false in $CONFIG_FILE.
Ensure $INSTALL_DIR precedes ~/.cargo/bin in PATH.
EOF
}

fail() {
  printf 'Error: %s\n' "$*" >&2
  if [[ "$HAS_CHANGED_CHANNELS" == 1 ]]; then
    handle_failure 1
  fi
  exit 1
}

get_binary_hash() {
  local checksum
  checksum="$(sha256sum < "$1")"
  printf '%s\n' "${checksum%% *}"
}

require_safe_paths() {
  local directory path index
  for directory in "$BUILDS_DIR" "$BUILDS_DIR/versions" \
    "$BUILDS_DIR/current" "$BUILDS_DIR/shared-server" "$INSTALL_DIR" "$BACKUPS_DIR"; do
    [[ ! -L "$directory" ]] || fail "Refusing symlinked directory: $directory"
    [[ ! -e "$directory" || -d "$directory" ]] || fail "Not a directory: $directory"
    mkdir -p "$directory"
  done
  for index in "${!PATHS[@]}"; do
    path="${PATHS[$index]}"
    if [[ "$index" == 2 || "$index" == 3 ]]; then
      [[ ! -L "$path" && ( ! -e "$path" || -f "$path" ) ]] ||
        fail "Refusing non-regular channel marker: $path"
    else
      [[ ! -e "$path" || -L "$path" ]] ||
        fail "Refusing to replace a non-symlink executable: $path"
    fi
  done
}

require_updates_disabled() {
  [[ -f "$CONFIG_FILE" ]] || fail "Set [features] check_updates = false in $CONFIG_FILE first."
  if ! awk '
    /^[[:space:]]*\[/ { in_features = ($0 ~ /^[[:space:]]*\[features\][[:space:]]*(#.*)?$/) }
    in_features && /^[[:space:]]*check_updates[[:space:]]*=/ {
      is_disabled = ($0 ~ /^[[:space:]]*check_updates[[:space:]]*=[[:space:]]*false[[:space:]]*(#.*)?$/)
    }
    END { exit !is_disabled }
  ' "$CONFIG_FILE"; then
    fail "Set [features] check_updates = false in $CONFIG_FILE first."
  fi
}

replace_link() {
  local target="$1" path="$2" staged="$2.ghe-install-$$"
  [[ ! -e "$staged" && ! -L "$staged" ]] || fail "Staging path already exists: $staged"
  ln -s -- "$target" "$staged"
  mv -Tf -- "$staged" "$path"
}

replace_marker() {
  local version="$1" path="$2" staged="$2.ghe-install-$$"
  [[ ! -e "$staged" && ! -L "$staged" ]] || fail "Staging path already exists: $staged"
  printf '%s\n' "$version" > "$staged"
  mv -Tf -- "$staged" "$path"
}

save_channels() {
  local index path
  BACKUP_DIR="$BACKUPS_DIR/$(date -u +%Y%m%dT%H%M%SZ)-$$"
  mkdir -- "$BACKUP_DIR"
  for index in "${!PATHS[@]}"; do
    path="${PATHS[$index]}"
    if [[ -e "$path" || -L "$path" ]]; then
      cp -P -- "$path" "$BACKUP_DIR/$index"
    fi
  done
  printf '%s\n' "$INSTALL_DIR" > "$BACKUP_DIR/install-directory"
  PREVIOUS_SERVER="$(readlink -f "$BUILDS_DIR/shared-server/jcode" || true)"
  if [[ ! -x "$PREVIOUS_SERVER" ]]; then
    PREVIOUS_SERVER="$(readlink -f "$BUILDS_DIR/stable/jcode" || true)"
  fi
  printf '%s\n' "$PREVIOUS_SERVER" > "$BACKUP_DIR/previous-server"
}

restore_channels() {
  local index path staged
  for index in "${!PATHS[@]}"; do
    path="${PATHS[$index]}"
    staged="$path.ghe-install-$$"
    if [[ -e "$BACKUP_DIR/$index" || -L "$BACKUP_DIR/$index" ]]; then
      cp -P -- "$BACKUP_DIR/$index" "$staged" || return
      mv -Tf -- "$staged" "$path" || return
    else
      rm -f -- "$path" || return
    fi
  done
}

reload_server() {
  local binary="$1"
  # Force bypasses same-version comparisons, but still uses graceful session handoff.
  # The daemon must not inherit the installer lock through a reload subprocess.
  JCODE_NO_AUTO_UPDATE=1 "$binary" --no-update server reload --force </dev/null 9>&-
}

handle_failure() {
  local status="$1"
  trap - ERR HUP INT TERM
  set +e
  if [[ "$HAS_CHANGED_CHANNELS" == 1 ]]; then
    printf 'Install failed; restoring the previous channels from %s.\n' "$BACKUP_DIR" >&2
    if restore_channels; then
      if [[ "$HAS_ATTEMPTED_RELOAD" == 1 && -x "$PREVIOUS_SERVER" ]]; then
        if ! reload_server "$PREVIOUS_SERVER"; then
          printf 'WARNING: old channels restored, but server handoff was not confirmed. Do not kill it; inspect server status.\n' >&2
        fi
      fi
    else
      printf 'WARNING: channel restoration failed. Recovery data: %s\n' "$BACKUP_DIR" >&2
    fi
  fi
  exit "$status"
}

cleanup() {
  local path
  [[ -z "$STAGING_DIR" ]] || rm -rf -- "$STAGING_DIR"
  for path in "${PATHS[@]}"; do
    rm -f -- "$path.ghe-install-$$"
  done
}

rollback_install() {
  local requested="$1" version backups_root
  backups_root="$(readlink -f -- "$BACKUPS_DIR")"
  BACKUP_DIR="$(readlink -f -- "$requested")"
  [[ "$BACKUP_DIR" == "$backups_root/"* && -d "$BACKUP_DIR" ]] ||
    fail "Backup must be inside $BACKUPS_DIR"
  [[ "$(cat "$BACKUP_DIR/install-directory")" == "$INSTALL_DIR" ]] ||
    fail "Backup belongs to a different JCODE_INSTALL_DIR."
  version="$(cat "$BACKUP_DIR/installed-version")"
  [[ "$version" =~ ^custom-ghe-[0-9a-f]{64}$ ]] || fail "Invalid backup version."
  [[ "$(cat "$BUILDS_DIR/current-version")" == "$version" &&
     "$(cat "$BUILDS_DIR/shared-server-version")" == "$version" &&
     "$(readlink "$BUILDS_DIR/current/jcode")" == "$BUILDS_DIR/versions/$version/jcode" &&
     "$(readlink "$BUILDS_DIR/shared-server/jcode")" == "$BUILDS_DIR/versions/$version/jcode" &&
     "$(readlink "$INSTALL_DIR/jcode")" == "$BUILDS_DIR/current/jcode" ]] ||
    fail "Channels changed since this install; refusing to overwrite newer state."
  PREVIOUS_SERVER="$(cat "$BACKUP_DIR/previous-server")"
  restore_channels
  if [[ -x "$PREVIOUS_SERVER" ]]; then
    reload_server "$PREVIOUS_SERVER" ||
      fail "Channels restored, but old server handoff was not confirmed; inspect status without killing it."
  fi
  printf 'Restored previous channels. Custom immutable payload retained: %s\n' "$version"
}

main() {
  local binary binary_tool checksum version version_dir builds_root install_root
  if [[ "${1:-}" == --help || "${1:-}" == -h ]]; then
    usage
    return
  fi
  if [[ "${1:-}" == --rollback ]]; then
    [[ "$#" == 2 ]] || { usage >&2; exit 1; }
  else
    [[ "$#" -le 1 ]] || { usage >&2; exit 1; }
    binary="${1:-$REPO_ROOT/target/selfdev/jcode}"
    [[ -f "$binary" && -x "$binary" ]] || fail "Already-built executable not found: $binary"
    binary="$(readlink -f -- "$binary")"
    require_updates_disabled
  fi
  [[ "$INSTALL_DIR" == /* ]] || fail "JCODE_INSTALL_DIR must be absolute."
  for binary_tool in flock sha256sum readlink install awk; do
    command -v "$binary_tool" >/dev/null || fail "Required tool not found: $binary_tool"
  done
  builds_root="$(readlink -m -- "$BUILDS_DIR")"
  install_root="$(readlink -m -- "$INSTALL_DIR")"
  [[ "$install_root" != "$builds_root" && "$install_root" != "$builds_root/"* ]] ||
    fail "JCODE_INSTALL_DIR must be outside the build store to preserve stable and immutable versions."
  require_safe_paths
  [[ ! -L "$BUILDS_DIR/.ghe-install.lock" &&
     ( ! -e "$BUILDS_DIR/.ghe-install.lock" || -f "$BUILDS_DIR/.ghe-install.lock" ) ]] ||
    fail "Refusing non-regular installer lock."
  exec 9>"$BUILDS_DIR/.ghe-install.lock"
  flock -n 9 || fail "Another GHE installer is active."
  trap cleanup EXIT
  if [[ "${1:-}" == --rollback ]]; then
    rollback_install "$2"
    return
  fi
  checksum="$(get_binary_hash "$binary")"
  version="custom-ghe-$checksum"
  version_dir="$BUILDS_DIR/versions/$version"
  [[ ! -L "$version_dir" ]] || fail "Refusing symlinked immutable version: $version_dir"
  if [[ -e "$version_dir" ]]; then
    [[ -f "$version_dir/jcode" && ! -L "$version_dir/jcode" && -x "$version_dir/jcode" ]] ||
      fail "Invalid existing immutable payload: $version_dir"
    [[ "$(get_binary_hash "$version_dir/jcode")" == "$checksum" ]] ||
      fail "Existing immutable payload does not match its hash: $version_dir"
  else
    STAGING_DIR="$BUILDS_DIR/versions/.ghe-install-$$"
    mkdir -- "$STAGING_DIR"
    install -m 755 -- "$binary" "$STAGING_DIR/jcode"
    [[ "$(get_binary_hash "$STAGING_DIR/jcode")" == "$checksum" ]] ||
      fail "Source binary changed during installation; retry after the build finishes."
    mv -T -- "$STAGING_DIR" "$version_dir"
    STAGING_DIR=""
  fi
  save_channels
  printf '%s\n' "$version" > "$BACKUP_DIR/installed-version"
  trap 'handle_failure $?' ERR
  trap 'handle_failure 129' HUP
  trap 'handle_failure 130' INT
  trap 'handle_failure 143' TERM
  HAS_CHANGED_CHANNELS=1
  replace_link "$version_dir/jcode" "$BUILDS_DIR/current/jcode"
  replace_marker "$version" "$BUILDS_DIR/current-version"
  replace_link "$version_dir/jcode" "$BUILDS_DIR/shared-server/jcode"
  replace_marker "$version" "$BUILDS_DIR/shared-server-version"
  replace_link "$BUILDS_DIR/current/jcode" "$INSTALL_DIR/jcode"
  HAS_ATTEMPTED_RELOAD=1
  reload_server "$version_dir/jcode"
  HAS_CHANGED_CHANNELS=0
  printf 'Installed: %s\nStable channel untouched. Shared server gracefully reloaded if active.\n' "$version_dir/jcode"
  printf 'Rollback: JCODE_INSTALL_DIR=%q bash %q --rollback %q\n' "$INSTALL_DIR" "$REPO_ROOT/scripts/install_ghe.sh" "$BACKUP_DIR"
  printf 'PATH: ensure %s precedes ~/.cargo/bin (no shell files changed).\n' "$INSTALL_DIR"
}

main "$@"
