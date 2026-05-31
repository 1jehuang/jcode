#!/usr/bin/env bash
# Safely reclaim disk space from the Cargo target directory.
#
# This is designed to be safe to run even while other builds are in progress on
# this machine (e.g. parallel self-dev agents). It will:
#   - never touch a target/<profile> dir that has an active rustc/cargo process
#     or that was written to within a recent activity window
#   - by default only remove cross-compile / compat caches and obviously stale
#     profile dirs, plus run `cargo clean` on stale profiles
#
# Usage:
#   scripts/clean_target.sh                 # dry-run: report what would be freed
#   scripts/clean_target.sh --apply         # actually delete safe items
#   scripts/clean_target.sh --apply --aggressive  # also sweep stale per-profile artifacts
#
# Env:
#   JCODE_CLEAN_ACTIVE_WINDOW_MIN  activity window in minutes (default 20)

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
apply="false"
aggressive="false"
activity_window_min="${JCODE_CLEAN_ACTIVE_WINDOW_MIN:-20}"

for arg in "$@"; do
  case "$arg" in
    --apply) apply="true" ;;
    --aggressive) aggressive="true" ;;
    -h|--help)
      sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      printf 'clean_target: unknown arg: %s\n' "$arg" >&2
      exit 2
      ;;
  esac
done

log() { printf 'clean_target: %s\n' "$*" >&2; }

human() {
  # Bytes -> human readable
  numfmt --to=iec --suffix=B "${1:-0}" 2>/dev/null || printf '%sB' "${1:-0}"
}

dir_bytes() {
  du -sb "$1" 2>/dev/null | awk '{print $1}'
}

# Is any rustc/cargo process currently operating inside this path?
path_has_active_process() {
  local path="$1"
  local p
  for p in $(pgrep -x rustc 2>/dev/null) $(pgrep -x cargo 2>/dev/null); do
    if tr '\0' ' ' < "/proc/$p/cmdline" 2>/dev/null | grep -qF "$path"; then
      return 0
    fi
  done
  return 1
}

# Was this path written to within the activity window?
path_recently_active() {
  local path="$1"
  [[ -d "$path" ]] || return 1
  local recent
  recent=$(find "$path" -maxdepth 3 -type f -newermt "-${activity_window_min} min" 2>/dev/null | head -1)
  [[ -n "$recent" ]]
}

is_safe_to_remove() {
  local path="$1"
  [[ -d "$path" ]] || return 1
  if path_has_active_process "$path"; then
    log "SKIP (active process): $path"
    return 1
  fi
  if path_recently_active "$path"; then
    log "SKIP (written <${activity_window_min}min ago): $path"
    return 1
  fi
  return 0
}

total_reclaimed=0

remove_path() {
  local path="$1" reason="$2"
  [[ -e "$path" ]] || return 0
  local bytes
  bytes=$(dir_bytes "$path")
  bytes=${bytes:-0}
  if ! is_safe_to_remove "$path"; then
    return 0
  fi
  if [[ "$apply" == "true" ]]; then
    if rm -rf "$path" 2>/dev/null; then
      log "removed ($reason): $path  [$(human "$bytes")]"
      total_reclaimed=$((total_reclaimed + bytes))
    else
      log "FAILED to remove (permissions? try sudo): $path  [$(human "$bytes")]"
    fi
  else
    log "would remove ($reason): $path  [$(human "$bytes")]"
    total_reclaimed=$((total_reclaimed + bytes))
  fi
}

log "target dir: $target_dir (activity window: ${activity_window_min}min, apply=$apply, aggressive=$aggressive)"

# 1) Cross-compile / compat caches: not part of the local dev inner loop. They
#    are regenerated on demand by release/compat scripts.
for d in "$target_dir"/*-apple-darwin "$target_dir"/*-pc-windows-* "$target_dir"/linux-compat; do
  [[ -d "$d" ]] || continue
  remove_path "$d" "cross-compile/compat cache"
done

# 2) Aggressive: cargo clean on stale (not-recently-active, no active process)
#    profiles to drop accumulated fingerprints/old artifact generations.
if [[ "$aggressive" == "true" ]]; then
  for profile_dir in "$target_dir"/debug "$target_dir"/release "$target_dir"/selfdev; do
    [[ -d "$profile_dir" ]] || continue
    profile=$(basename "$profile_dir")
    [[ "$profile" == "debug" ]] && profile="dev"
    if ! is_safe_to_remove "$profile_dir"; then
      continue
    fi
    before=$(dir_bytes "$profile_dir"); before=${before:-0}
    if [[ "$apply" == "true" ]]; then
      log "cargo clean --profile $profile (stale) ..."
      cargo clean --profile "$profile" 2>/dev/null || log "  cargo clean failed for $profile"
      after=$(dir_bytes "$profile_dir"); after=${after:-0}
      freed=$((before - after))
      (( freed > 0 )) && total_reclaimed=$((total_reclaimed + freed))
      log "  freed $(human "$freed") from $profile"
    else
      log "would cargo clean --profile $profile  [up to $(human "$before")]"
      total_reclaimed=$((total_reclaimed + before))
    fi
  done
fi

log "total $([ "$apply" == true ] && echo reclaimed || echo reclaimable): $(human "$total_reclaimed")"
