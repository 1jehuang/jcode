#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

log() {
  printf 'dev_cargo: %s\n' "$*" >&2
}

selected_linker_mode="not-configured"
selected_linker_desc=""
sccache_status="disabled"
selfdev_low_memory_status="disabled"
feature_profile_status="default"

append_rustflags() {
  local new_flag="$1"
  if [[ -z "${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS:-}" ]]; then
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="$new_flag"
  else
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS} ${new_flag}"
  fi
}

maybe_enable_sccache() {
  if [[ -n "${RUSTC_WRAPPER:-}" ]]; then
    sccache_status="external:${RUSTC_WRAPPER}"
    log "keeping existing RUSTC_WRAPPER=${RUSTC_WRAPPER}"
    return
  fi
  if command -v sccache >/dev/null 2>&1; then
    sccache --start-server >/dev/null 2>&1 || true
    export RUSTC_WRAPPER=sccache
    sccache_status="enabled"
    log "using sccache"
  else
    sccache_status="not-found"
    log "sccache not found; using direct rustc"
  fi
}

uses_selfdev_profile() {
  local expect_profile_name="false"
  for arg in "$@"; do
    if [[ "$expect_profile_name" == "true" ]]; then
      [[ "$arg" == "selfdev" ]] && return 0
      expect_profile_name="false"
      continue
    fi

    case "$arg" in
      --profile=selfdev)
        return 0
        ;;
      --profile)
        expect_profile_name="true"
        ;;
    esac
  done
  return 1
}

has_explicit_feature_args() {
  local expect_value="false"
  for arg in "$@"; do
    if [[ "$expect_value" == "true" ]]; then
      expect_value="false"
      continue
    fi
    case "$arg" in
      --)
        return 1
        ;;
      --features|--no-default-features)
        return 0
        ;;
      --features=*|--no-default-features=*)
        return 0
        ;;
    esac
  done
  return 1
}

feature_args_from_profile() {
  local profile="$1"
  case "$profile" in
    ""|default)
      return 0
      ;;
    minimal|none)
      printf '%s\0' --no-default-features
      ;;
    pdf)
      printf '%s\0' --no-default-features --features pdf
      ;;
    embeddings)
      printf '%s\0' --no-default-features --features embeddings
      ;;
    full)
      printf '%s\0' --features embeddings,pdf
      ;;
    *)
      return 1
      ;;
  esac
}

validate_feature_profile() {
  local profile="${JCODE_DEV_FEATURE_PROFILE:-default}"
  case "$profile" in
    ""|default|minimal|none|pdf|embeddings|full)
      ;;
    *)
      printf 'error: unsupported JCODE_DEV_FEATURE_PROFILE=%s (expected default|minimal|pdf|embeddings|full)\n' "$profile" >&2
      exit 1
      ;;
  esac
}

build_cargo_argv() {
  local profile="${JCODE_DEV_FEATURE_PROFILE:-default}"
  if [[ "$profile" == "default" || -z "$profile" ]]; then
    feature_profile_status="default"
    printf '%s\0' "$@"
    return 0
  fi

  if has_explicit_feature_args "$@"; then
    feature_profile_status="ignored-explicit-cargo-args"
    printf '%s\0' "$@"
    return 0
  fi

  local -a feature_args=()
  while IFS= read -r -d '' arg; do
    feature_args+=("$arg")
  done < <(feature_args_from_profile "$profile")

  feature_profile_status="$profile"
  local inserted="false"
  for arg in "$@"; do
    if [[ "$arg" == "--" && "$inserted" == "false" ]]; then
      printf '%s\0' "${feature_args[@]}"
      inserted="true"
    fi
    printf '%s\0' "$arg"
  done
  if [[ "$inserted" == "false" ]]; then
    printf '%s\0' "${feature_args[@]}"
  fi
}

meminfo_kib() {
  local key="$1"
  awk -v key="$key" '$1 == key ":" { print $2; exit }' /proc/meminfo 2>/dev/null || true
}

selfdev_low_memory_default_needed() {
  [[ "$(uname -s)" == "Linux" ]] || return 1
  [[ -r /proc/meminfo ]] || return 1
  command -v pgrep >/dev/null 2>&1 || return 1
  pgrep -x earlyoom >/dev/null 2>&1 || return 1

  local mem_total_kib swap_total_kib
  mem_total_kib=$(meminfo_kib MemTotal)
  swap_total_kib=$(meminfo_kib SwapTotal)
  [[ -n "$mem_total_kib" && -n "$swap_total_kib" ]] || return 1

  # On small no-swap machines, earlyoom can terminate the root jcode rustc
  # around 1 GiB RSS before the kernel OOM killer would report anything.
  # Keep this adaptive so larger workstations retain the faster inherited
  # selfdev profile by default.
  (( swap_total_kib == 0 && mem_total_kib < 24576 * 1024 ))
}

maybe_configure_low_memory_selfdev() {
  if ! uses_selfdev_profile "$@"; then
    selfdev_low_memory_status="not-selfdev"
    return
  fi

  local mode="${JCODE_SELFDEV_LOW_MEMORY:-auto}"
  case "$mode" in
    1|true|yes|on|force)
      ;;
    0|false|no|off|never)
      selfdev_low_memory_status="disabled-by-env"
      return
      ;;
    auto|"")
      if ! selfdev_low_memory_default_needed; then
        selfdev_low_memory_status="auto-not-needed"
        return
      fi
      ;;
    *)
      printf 'error: unsupported JCODE_SELFDEV_LOW_MEMORY=%s (expected auto|on|off)\n' "$mode" >&2
      exit 1
      ;;
  esac

  export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
  export CARGO_PROFILE_SELFDEV_INCREMENTAL="${CARGO_PROFILE_SELFDEV_INCREMENTAL:-false}"
  export CARGO_PROFILE_SELFDEV_CODEGEN_UNITS="${CARGO_PROFILE_SELFDEV_CODEGEN_UNITS:-16}"
  selfdev_low_memory_status="enabled:incremental=${CARGO_PROFILE_SELFDEV_INCREMENTAL},codegen-units=${CARGO_PROFILE_SELFDEV_CODEGEN_UNITS}"
  log "using low-memory selfdev overrides (${selfdev_low_memory_status#enabled:})"
}

configure_linux_linker() {
  local requested_mode="${JCODE_FAST_LINKER:-auto}"
  local mode="$requested_mode"

  case "$mode" in
    auto)
      if command -v ld.lld >/dev/null 2>&1 && command -v clang >/dev/null 2>&1; then
        mode="lld"
      elif command -v mold >/dev/null 2>&1 && command -v clang >/dev/null 2>&1; then
        mode="mold"
      else
        mode="system"
      fi
      ;;
    lld|mold|system)
      ;;
    *)
      printf 'error: unsupported JCODE_FAST_LINKER=%s (expected auto|lld|mold|system)\n' "$mode" >&2
      exit 1
      ;;
  esac

  selected_linker_mode="$mode"
  export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-clang}"

  case "$mode" in
    lld)
      append_rustflags "-C link-arg=-fuse-ld=lld"
      selected_linker_desc="clang + lld"
      log "using clang + lld"
      ;;
    mold)
      append_rustflags "-C link-arg=-fuse-ld=mold"
      selected_linker_desc="clang + mold"
      log "using clang + mold"
      ;;
    system)
      selected_linker_desc="system linker settings"
      if [[ "$requested_mode" == "auto" ]]; then
        log "no supported fast linker detected; using system linker settings"
      else
        log "using system linker settings"
      fi
      ;;
  esac
}

print_setup() {
  if [[ -n "${JCODE_DEV_FEATURE_PROFILE:-}" && "${JCODE_DEV_FEATURE_PROFILE}" != "default" ]]; then
    feature_profile_status="${JCODE_DEV_FEATURE_PROFILE}"
  fi
  cat <<EOF
repo_root=$repo_root
os=$(uname -s)
arch=$(uname -m)
sccache_status=$sccache_status
selfdev_low_memory_status=$selfdev_low_memory_status
feature_profile_status=$feature_profile_status
rustc_wrapper=${RUSTC_WRAPPER:-<unset>}
linker_mode=$selected_linker_mode
linker_desc=${selected_linker_desc:-<none>}
linker=${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-<unset>}
rustflags=${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS:-<unset>}
EOF
}

validate_feature_profile
maybe_configure_low_memory_selfdev "$@"
maybe_enable_sccache

if [[ "$(uname -s)" == "Linux" ]] && [[ "$(uname -m)" == "x86_64" ]]; then
  configure_linux_linker
fi

if [[ "${1:-}" == "--print-setup" ]]; then
  print_setup
  exit 0
fi

cargo_argv=()
while IFS= read -r -d '' arg; do
  cargo_argv+=("$arg")
done < <(build_cargo_argv "$@")

exec cargo "${cargo_argv[@]}"
