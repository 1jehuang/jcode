#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_ROOT="$REPO_ROOT/.ghe-installer-test-$$"
INSTALLER="$REPO_ROOT/scripts/install_ghe.sh"
readonly REPO_ROOT FIXTURE_ROOT INSTALLER
checksum() { shasum -a 256 "$@"; }
inode() {
  if [[ "$(uname -s)" == Darwin ]]; then stat -Lf %i "$1"; else stat -Lc %i "$1"; fi
}
[[ ! -e "$FIXTURE_ROOT" ]] || { echo "Fixture already exists" >&2; exit 1; }
mkdir "$FIXTURE_ROOT"
trap 'rm -rf -- "$FIXTURE_ROOT"' EXIT

create_fixture() {
  FIXTURE="$FIXTURE_ROOT/$1"
  HOME_DIR="$FIXTURE/home"
  BUILDS="$HOME_DIR/.jcode/builds"
  mkdir -p "$BUILDS/versions/0.91.0" "$BUILDS/stable" \
    "$BUILDS/current" "$BUILDS/shared-server" "$HOME_DIR/.local/bin"
  cat > "$BUILDS/versions/0.91.0/jcode" <<'EOF'
#!/usr/bin/env bash
printf 'stock:%s:%s\n' "${JCODE_NO_AUTO_UPDATE:-}" "$*" >> "$TEST_HOME/reloads"
EOF
  cat > "$FIXTURE/custom-jcode" <<'EOF'
#!/usr/bin/env bash
printf 'custom:%s:%s\n' "${JCODE_NO_AUTO_UPDATE:-}" "$*" >> "$TEST_HOME/reloads"
[[ "${TEST_RELOAD_FAIL:-0}" != 1 ]]
EOF
  chmod +x "$BUILDS/versions/0.91.0/jcode" "$FIXTURE/custom-jcode"
  ln -s "$BUILDS/versions/0.91.0/jcode" "$BUILDS/stable/jcode"
  ln -s "$BUILDS/versions/0.91.0/jcode" "$BUILDS/current/jcode"
  ln -s "$BUILDS/versions/0.91.0/jcode" "$BUILDS/shared-server/jcode"
  ln -s "$BUILDS/stable/jcode" "$HOME_DIR/.local/bin/jcode"
  printf '0.91.0\n' > "$BUILDS/stable-version"
  printf '0.91.0\n' > "$BUILDS/current-version"
  printf '0.91.0\n' > "$BUILDS/shared-server-version"
  printf '[features]\ncheck_updates = false # keep the custom build pinned\n' > "$HOME_DIR/.jcode/config.toml"
  printf 'original shell setup\n' > "$HOME_DIR/.bashrc"
}

run_install() {
  HOME="$HOME_DIR" TEST_HOME="$HOME_DIR" JCODE_INSTALL_DIR="$HOME_DIR/.local/bin" \
    bash "$INSTALLER" "$FIXTURE/custom-jcode" > "$FIXTURE/output" 2> "$FIXTURE/errors"
}

run_rollback() {
  local backup
  backup="$(find "$BUILDS/ghe-install-backups" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  HOME="$HOME_DIR" TEST_HOME="$HOME_DIR" JCODE_INSTALL_DIR="$HOME_DIR/.local/bin" \
    bash "$INSTALLER" --rollback "$backup" > "$FIXTURE/output" 2> "$FIXTURE/errors"
}

assert_equal() {
  [[ "$1" == "$2" ]] || { printf 'Expected: %s\nActual: %s\n' "$1" "$2" >&2; exit 1; }
}

should_preserve_stable_symlink() {
  create_fixture stable-symlink
  run_install
  assert_equal "$BUILDS/versions/0.91.0/jcode" "$(readlink "$BUILDS/stable/jcode")"
}

should_preserve_stock_payload() {
  create_fixture stock-payload
  local original
  original="$(checksum "$BUILDS/versions/0.91.0/jcode")"
  run_install
  assert_equal "$original" "$(checksum "$BUILDS/versions/0.91.0/jcode")"
}

should_preserve_stable_marker() {
  create_fixture stable-marker
  run_install
  assert_equal 0.91.0 "$(cat "$BUILDS/stable-version")"
}

should_point_launcher_at_current() {
  create_fixture launcher
  run_install
  assert_equal "$BUILDS/current/jcode" "$(readlink "$HOME_DIR/.local/bin/jcode")"
}

should_share_custom_payload_between_channels() {
  create_fixture channel-payloads
  run_install
  assert_equal "$(readlink "$BUILDS/current/jcode")" "$(readlink "$BUILDS/shared-server/jcode")"
}

should_share_version_markers_between_channels() {
  create_fixture channel-markers
  run_install
  assert_equal "$(cat "$BUILDS/current-version")" "$(cat "$BUILDS/shared-server-version")"
}

should_address_payload_by_content_hash() {
  create_fixture content-hash
  local checksum
  checksum="$(checksum < "$FIXTURE/custom-jcode")"
  run_install
  assert_equal "custom-ghe-${checksum%% *}" "$(cat "$BUILDS/current-version")"
}

should_reload_without_updates_using_forced_handoff() {
  create_fixture reload
  run_install
  assert_equal 'custom:1:--no-update server reload --force' "$(cat "$HOME_DIR/reloads")"
}

should_reuse_identical_immutable_payload() {
  create_fixture reuse
  run_install
  local inode
  inode="$(inode "$BUILDS/current/jcode")"
  run_install
  assert_equal "$inode" "$(inode "$BUILDS/current/jcode")"
}

should_leave_configuration_unchanged() {
  create_fixture config
  local original
  original="$(checksum "$HOME_DIR/.jcode/config.toml")"
  run_install
  assert_equal "$original" "$(checksum "$HOME_DIR/.jcode/config.toml")"
}

should_leave_shell_startup_unchanged() {
  create_fixture shell
  run_install
  assert_equal 'original shell setup' "$(cat "$HOME_DIR/.bashrc")"
}

should_restore_shared_channel_after_reload_failure() {
  create_fixture reload-failure-channel
  if TEST_RELOAD_FAIL=1 run_install; then
    echo "Expected reload failure" >&2
    exit 1
  fi
  assert_equal "$BUILDS/versions/0.91.0/jcode" "$(readlink "$BUILDS/shared-server/jcode")"
}

should_restore_marker_after_reload_failure() {
  create_fixture reload-failure-marker
  if TEST_RELOAD_FAIL=1 run_install; then
    echo "Expected reload failure" >&2
    exit 1
  fi
  assert_equal 0.91.0 "$(cat "$BUILDS/current-version")"
}

should_reload_previous_server_after_failure() {
  create_fixture reload-failure-server
  if TEST_RELOAD_FAIL=1 run_install; then
    echo "Expected reload failure" >&2
    exit 1
  fi
  assert_equal $'custom:1:--no-update server reload --force\nstock:1:--no-update server reload --force' "$(cat "$HOME_DIR/reloads")"
}

should_restore_launcher_on_explicit_rollback() {
  create_fixture rollback
  run_install
  run_rollback
  assert_equal "$BUILDS/stable/jcode" "$(readlink "$HOME_DIR/.local/bin/jcode")"
}

should_reject_rollback_after_channels_change() {
  create_fixture rollback-newer
  run_install
  printf 'newer-version\n' > "$BUILDS/current-version"
  if run_rollback; then
    echo "Expected rejection of stale rollback" >&2
    exit 1
  fi
  assert_equal newer-version "$(cat "$BUILDS/current-version")"
}

should_reject_regular_launcher() {
  create_fixture regular-launcher
  rm "$HOME_DIR/.local/bin/jcode"
  printf 'user executable\n' > "$HOME_DIR/.local/bin/jcode"
  if run_install; then
    echo "Expected rejection of a regular launcher" >&2
    exit 1
  fi
  assert_equal 'user executable' "$(cat "$HOME_DIR/.local/bin/jcode")"
}

should_require_persistent_update_disable() {
  create_fixture enabled-updates
  printf '[features]\ncheck_updates = true\n' > "$HOME_DIR/.jcode/config.toml"
  if run_install; then
    echo "Expected rejection of automatic updates" >&2
    exit 1
  fi
  assert_equal 0.91.0 "$(cat "$BUILDS/current-version")"
}

should_reject_modified_immutable_payload() {
  create_fixture modified-payload
  run_install
  printf 'unexpected bytes\n' >> "$(readlink "$BUILDS/current/jcode")"
  if run_install; then
    echo "Expected rejection of modified immutable payload" >&2
    exit 1
  fi
  grep -q 'does not match its hash' "$FIXTURE/errors"
}

should_reject_concurrent_installer() {
  create_fixture installer-lock
  mkdir "$BUILDS/.ghe-install.lock.d"
  if run_install; then
    echo "Expected rejection of concurrent installer" >&2
    exit 1
  fi
  rmdir "$BUILDS/.ghe-install.lock.d"
  assert_equal 0.91.0 "$(cat "$BUILDS/current-version")"
}

should_reject_launcher_inside_build_store() {
  create_fixture launcher-in-stable
  if HOME="$HOME_DIR" JCODE_INSTALL_DIR="$BUILDS/stable" \
    bash "$INSTALLER" "$FIXTURE/custom-jcode" > "$FIXTURE/output" 2> "$FIXTURE/errors"; then
    echo "Expected rejection of a launcher inside the stable channel" >&2
    exit 1
  fi
  assert_equal "$BUILDS/versions/0.91.0/jcode" "$(readlink "$BUILDS/stable/jcode")"
}

should_reject_symlinked_current_directory() {
  create_fixture current-directory
  rm "$BUILDS/current/jcode"
  rmdir "$BUILDS/current"
  ln -s "$BUILDS/stable" "$BUILDS/current"
  if run_install; then
    echo "Expected rejection of current directory aliased to stable" >&2
    exit 1
  fi
  assert_equal "$BUILDS/versions/0.91.0/jcode" "$(readlink "$BUILDS/stable/jcode")"
}

should_not_pass_installer_lock_to_server() {
  create_fixture lock-inheritance
  printf '%s\n' 'test ! -e "/proc/$$/fd/9"' >> "$FIXTURE/custom-jcode"
  run_install
  assert_equal 'custom:1:--no-update server reload --force' "$(cat "$HOME_DIR/reloads")"
}

should_reject_missing_rollback_payload_before_any_mutation() {
  local legacy="$1" version backup
  create_fixture "missing-payload-$legacy"
  run_install
  version="$(cat "$BUILDS/current-version")"
  backup="$(find "$BUILDS/ghe-install-backups" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  if [[ "$legacy" == legacy ]]; then
    rm "$backup/target-0" "$backup/target-1" "$backup/target-4"
  fi
  rm "$BUILDS/versions/0.91.0/jcode"
  if run_rollback; then
    echo 'Expected rollback rejection for missing payload' >&2; exit 1
  fi
  assert_equal "$version" "$(cat "$BUILDS/current-version")"
  assert_equal "$version" "$(cat "$BUILDS/shared-server-version")"
  assert_equal "$BUILDS/versions/$version/jcode" "$(readlink "$BUILDS/current/jcode")"
  assert_equal "$BUILDS/versions/$version/jcode" "$(readlink "$BUILDS/shared-server/jcode")"
  assert_equal "$BUILDS/current/jcode" "$(readlink "$HOME_DIR/.local/bin/jcode")"
  assert_equal 'custom:1:--no-update server reload --force' "$(cat "$HOME_DIR/reloads")"
}

should_record_resolved_backup_targets() {
  create_fixture backup-targets
  # Resolve relative channel links at their original location, not the backup.
  rm "$BUILDS/current/jcode"
  ln -s ../versions/0.91.0/jcode "$BUILDS/current/jcode"
  run_install
  local backup
  backup="$(find "$BUILDS/ghe-install-backups" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  for index in 0 1 4; do
    assert_equal "$BUILDS/versions/0.91.0/jcode" "$(cat "$backup/target-$index")"
  done
  run_rollback
  assert_equal ../versions/0.91.0/jcode "$(readlink "$BUILDS/current/jcode")"
  test -x "$BUILDS/current/jcode"
}

should_preflight_later_rollback_targets() {
  local kind="$1" missing version
  create_fixture "missing-later-$kind"
  mkdir "$BUILDS/versions/old-independent"
  cp "$BUILDS/versions/0.91.0/jcode" "$BUILDS/versions/old-independent/jcode"
  if [[ "$kind" == launcher ]]; then
    missing="$BUILDS/versions/old-independent/jcode"
    rm "$HOME_DIR/.local/bin/jcode"
    ln -s "$missing" "$HOME_DIR/.local/bin/jcode"
  else
    # With no shared channel, stable is the fallback previous server. Keep
    # current and launcher on a different payload so only that fallback is lost.
    missing="$BUILDS/versions/0.91.0/jcode"
    rm "$BUILDS/shared-server/jcode" "$BUILDS/shared-server-version" \
      "$BUILDS/current/jcode" "$HOME_DIR/.local/bin/jcode"
    ln -s "$BUILDS/versions/old-independent/jcode" "$BUILDS/current/jcode"
    ln -s "$BUILDS/current/jcode" "$HOME_DIR/.local/bin/jcode"
    printf 'old-independent\n' > "$BUILDS/current-version"
  fi
  run_install
  version="$(cat "$BUILDS/current-version")"
  rm "$missing"
  if run_rollback; then
    echo 'Expected preflight rejection before restoring any channel' >&2; exit 1
  fi
  assert_equal "$version" "$(cat "$BUILDS/current-version")"
  assert_equal "$version" "$(cat "$BUILDS/shared-server-version")"
  assert_equal "$BUILDS/versions/$version/jcode" "$(readlink "$BUILDS/current/jcode")"
  assert_equal "$BUILDS/versions/$version/jcode" "$(readlink "$BUILDS/shared-server/jcode")"
  assert_equal "$BUILDS/current/jcode" "$(readlink "$HOME_DIR/.local/bin/jcode")"
}

should_preserve_stable_symlink
should_preserve_stock_payload
should_preserve_stable_marker
should_point_launcher_at_current
should_share_custom_payload_between_channels
should_share_version_markers_between_channels
should_address_payload_by_content_hash
should_reload_without_updates_using_forced_handoff
should_reuse_identical_immutable_payload
should_leave_configuration_unchanged
should_leave_shell_startup_unchanged
should_restore_shared_channel_after_reload_failure
should_restore_marker_after_reload_failure
should_reload_previous_server_after_failure
should_restore_launcher_on_explicit_rollback
should_reject_rollback_after_channels_change
should_reject_regular_launcher
should_require_persistent_update_disable
should_reject_modified_immutable_payload
should_reject_concurrent_installer
should_reject_launcher_inside_build_store
should_reject_symlinked_current_directory
should_not_pass_installer_lock_to_server
should_reject_missing_rollback_payload_before_any_mutation new
should_reject_missing_rollback_payload_before_any_mutation legacy
should_record_resolved_backup_targets
should_preflight_later_rollback_targets launcher
should_preflight_later_rollback_targets server
printf 'install_ghe: 28 regression tests passed\n'
