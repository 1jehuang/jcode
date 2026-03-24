#!/usr/bin/env bash
set -euo pipefail

# Keep files created by this helper private by default.
umask 077

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
user_name="${USER:-$(id -un)}"
runtime_dir="${XDG_RUNTIME_DIR:-/tmp}"
default_home="${HOME}/.jcode-refactor"
default_socket="${runtime_dir}/jcode-refactor-${user_name}.sock"

ref_home="${JCODE_REF_HOME:-$default_home}"
ref_socket="${JCODE_REF_SOCKET:-$default_socket}"
ref_profile="${JCODE_REF_PROFILE:-debug}"

case "$ref_profile" in
  debug) default_bin="$repo_root/target/debug/jcode" ;;
  release) default_bin="$repo_root/target/release/jcode" ;;
  *)
    printf 'error: unsupported JCODE_REF_PROFILE: %s (expected debug or release)\n' "$ref_profile" >&2
    exit 1
    ;;
esac

ref_bin="${JCODE_REF_BIN:-$default_bin}"

usage() {
  cat <<'USAGE'
Usage:
  scripts/refactor_shadow.sh env
  scripts/refactor_shadow.sh build [--release]
  scripts/refactor_shadow.sh serve [-- <jcode serve args>]
  scripts/refactor_shadow.sh run [-- <jcode args>]
  scripts/refactor_shadow.sh connect [-- <jcode connect args>]
  scripts/refactor_shadow.sh check

What it does:
  - Runs jcode in an isolated refactor environment
  - Uses separate JCODE_HOME and JCODE_SOCKET
  - Refuses to run against ~/.jcode to protect live sessions

Environment overrides:
  JCODE_REF_HOME      Isolated home dir (default: ~/.jcode-refactor)
  JCODE_REF_SOCKET    Isolated socket path
  JCODE_REF_PROFILE   debug|release (default: debug)
  JCODE_REF_BIN       Explicit jcode binary path
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

assert_safe_paths() {
  [[ -n "$ref_home" ]] || die "JCODE_REF_HOME resolved to empty path"
  [[ -n "$ref_socket" ]] || die "JCODE_REF_SOCKET resolved to empty path"
  [[ "$ref_home" = /* ]] || die "JCODE_REF_HOME must be an absolute path: $ref_home"
  [[ "$ref_socket" = /* ]] || die "JCODE_REF_SOCKET must be an absolute path: $ref_socket"

  local prod_home="${HOME}/.jcode"
  if [[ "$ref_home" == "$prod_home" ]]; then
    die "refusing to run with production home ($prod_home); set JCODE_REF_HOME to an isolated path"
  fi
}

ensure_ref_home() {
  if [[ ! -d "$ref_home" ]]; then
    mkdir -p -m 700 "$ref_home"
  fi
  # Best-effort hardening if dir already exists.
  chmod 700 "$ref_home" 2>/dev/null || true
}

ensure_socket_parent() {
  local socket_parent
  socket_parent=$(dirname "$ref_socket")
  if [[ ! -d "$socket_parent" ]]; then
    mkdir -p -m 700 "$socket_parent"
  fi
}

ensure_binary() {
  if [[ ! -x "$ref_bin" ]]; then
    die "jcode binary not found or not executable: $ref_bin (run 'scripts/refactor_shadow.sh build')"
  fi
}

remove_stale_socket() {
  local debug_socket
  debug_socket="${ref_socket%.sock}-debug.sock"
  for path in "$ref_socket" "$debug_socket"; do
    if [[ -e "$path" ]]; then
      if [[ -S "$path" ]]; then
        rm -f "$path"
      else
        die "refusing to remove non-socket path: $path"
      fi
    fi
  done
}

run_isolated() {
  JCODE_HOME="$ref_home" JCODE_SOCKET="$ref_socket" "$@"
}

normalize_args() {
  if [[ "${1:-}" == "--" ]]; then
    shift
  fi
  printf '%s\0' "$@"
}

cmd_env() {
  cat <<EOF_OUT
JCODE_REF_HOME=$ref_home
JCODE_REF_SOCKET=$ref_socket
JCODE_REF_PROFILE=$ref_profile
JCODE_REF_BIN=$ref_bin

# One-off command example:
JCODE_HOME=$ref_home JCODE_SOCKET=$ref_socket $ref_bin --version
EOF_OUT
}

cmd_build() {
  local profile_flag=""
  if [[ "${1:-}" == "--release" ]]; then
    profile_flag="--release"
  elif [[ -n "${1:-}" ]]; then
    die "unknown build argument: $1"
  fi

  (cd "$repo_root" && "$repo_root/scripts/dev_cargo.sh" build $profile_flag)
}

cmd_check() {
  assert_safe_paths
  ensure_ref_home
  ensure_socket_parent

  printf 'Refactor home:    %s\n' "$ref_home"
  printf 'Refactor socket:  %s\n' "$ref_socket"
  printf 'Refactor binary:  %s\n' "$ref_bin"

  if [[ -S "$ref_socket" ]]; then
    printf 'Socket status:    present (server likely running)\n'
  elif [[ -e "$ref_socket" ]]; then
    printf 'Socket status:    present but not a socket (unexpected)\n'
    exit 1
  else
    printf 'Socket status:    not present\n'
  fi
}

cmd_serve() {
  assert_safe_paths
  ensure_ref_home
  ensure_socket_parent
  ensure_binary
  remove_stale_socket

  local -a args=("$@")
  run_isolated "$ref_bin" serve "${args[@]}"
}

cmd_run() {
  assert_safe_paths
  ensure_ref_home
  ensure_socket_parent
  ensure_binary

  local -a args=("$@")
  run_isolated "$ref_bin" "${args[@]}"
}

cmd_connect() {
  assert_safe_paths
  ensure_ref_home
  ensure_socket_parent
  ensure_binary

  local -a args=("$@")
  run_isolated "$ref_bin" connect "${args[@]}"
}

main() {
  local cmd="${1:-help}"
  shift || true

  case "$cmd" in
    env)
      cmd_env
      ;;
    build)
      cmd_build "$@"
      ;;
    serve)
      if [[ "${1:-}" == "--" ]]; then
        shift
      fi
      cmd_serve "$@"
      ;;
    run)
      if [[ "${1:-}" == "--" ]]; then
        shift
      fi
      cmd_run "$@"
      ;;
    connect)
      if [[ "${1:-}" == "--" ]]; then
        shift
      fi
      cmd_connect "$@"
      ;;
    check)
      cmd_check
      ;;
    help|-h|--help)
      usage
      ;;
    *)
      die "unknown command: $cmd (use --help)"
      ;;
  esac
}

main "$@"
