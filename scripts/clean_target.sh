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
#   scripts/clean_target.sh --sweep 7       # also sweep stale artifact generations
#                                           # older than 7 days (keeps the newest
#                                           # generation and incremental cache per
#                                           # crate, so the warm cache survives;
#                                           # dry-run w/o --apply)
#   scripts/clean_target.sh --apply --aggressive  # cargo clean stale profiles
#   scripts/clean_target.sh --apply --sweep 2 --sweep-only  # only the sweep,
#                                           # keep cross-compile caches
#
# Works with GNU (Linux) and BSD (macOS) userlands.
#
# Env:
#   JCODE_CLEAN_ACTIVE_WINDOW_MIN  activity window in minutes (default 20)

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
apply="false"
aggressive="false"
sweep_only="false"
sweep_days=""
activity_window_min="${JCODE_CLEAN_ACTIVE_WINDOW_MIN:-20}"

expect_sweep_days="false"
for arg in "$@"; do
  if [[ "$expect_sweep_days" == "true" ]]; then
    sweep_days="$arg"
    expect_sweep_days="false"
    continue
  fi
  case "$arg" in
    --apply) apply="true" ;;
    --aggressive) aggressive="true" ;;
    --sweep-only) sweep_only="true" ;;
    --sweep) expect_sweep_days="true" ;;
    --sweep=*) sweep_days="${arg#--sweep=}" ;;
    -h|--help)
      sed -n '2,29p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      printf 'clean_target: unknown arg: %s\n' "$arg" >&2
      exit 2
      ;;
  esac
done
if [[ "$expect_sweep_days" == "true" ]]; then
  printf 'clean_target: --sweep requires a day count\n' >&2
  exit 2
fi
if [[ -n "$sweep_days" && ! "$sweep_days" =~ ^[0-9]+$ ]]; then
  printf 'clean_target: --sweep expects a whole number of days, got: %s\n' "$sweep_days" >&2
  exit 2
fi

log() { printf 'clean_target: %s\n' "$*" >&2; }

# Paths never enter whitespace-delimited stat/sort/awk records.
if stat -c '%Y' / >/dev/null 2>&1; then
  stat_fmt=(-c '%Y %s')
else
  stat_fmt=(-f '%m %z')
fi

# Resolve the trusted root once. Do not create a missing target, or allow a
# filesystem root to become a recursive deletion boundary. The suffix prevents
# command substitution from stripping newlines from a directory name.
[[ -d "$target_dir" ]] || { log "no target directory: $target_dir"; exit 0; }
target_dir=$(cd -P -- "$target_dir" && printf '%s/' "$PWD")
target_dir=${target_dir%/}
[[ "$target_dir" != / ]] || { log "refusing filesystem root"; exit 2; }

path_is_contained() {
  local path="$1" resolved parent
  # Do not remove symlinks even when they currently point inside the target.
  [[ ! -L "$path" ]] || { log "SKIP (symlink): $path"; return 1; }
  if [[ -d "$path" ]]; then
    resolved=$(cd -P -- "$path" && printf '%s/' "$PWD") || return 1
    resolved=${resolved%/}
  else
    parent=$(cd -P -- "${path%/*}" && printf '%s/' "$PWD") || return 1
    resolved="${parent}${path##*/}"
  fi
  case "$resolved" in
    "$target_dir"/*) return 0 ;;
    *) log "SKIP (outside canonical target): $path"; return 1 ;;
  esac
}

# Epoch seconds `days` days ago (no GNU `date -d`).
cutoff_epoch() {
  echo $(( $(date +%s) - $1 * 86400 ))
}

human() {
  # Bytes -> human readable
  numfmt --to=iec --suffix=B "${1:-0}" 2>/dev/null || printf '%sB' "${1:-0}"
}

dir_bytes() {
  local kib
  kib=$(du -sk "$1" 2>/dev/null | awk 'NR == 1 {print $1}')
  echo $(( ${kib:-0} * 1024 ))
}

# Is any rustc/cargo process currently operating inside this path?
path_has_active_process() {
  local path="$1" procs
  procs=$(ps -A -o command= 2>/dev/null) || return 1
  # One awk pass: an early-exiting `grep -q` in a pipeline would SIGPIPE its
  # writer, and with pipefail that reads as "no match".
  awk -v path="$path" '
    $0 ~ /(^|\/)(rustc|cargo|clippy-driver)( |$)/ && index($0, path) { found = 1 }
    END { exit !found }' <<<"$procs"
}

# Was this path written to within the activity window?
path_recently_active() {
  local path="$1"
  [[ -d "$path" ]] || return 1
  local recent
  recent=$(find "$path" -maxdepth 3 -type f -mmin "-${activity_window_min}" 2>/dev/null | head -1)
  [[ -n "$recent" ]]
}

is_safe_to_remove() {
  local path="$1"
  [[ -d "$path" ]] || return 1
  path_is_contained "$path" || return 1
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
    if path_is_contained "$path" && rm -rf -- "$path"; then
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

log "target dir: $target_dir (activity window: ${activity_window_min}min, apply=$apply, aggressive=$aggressive, sweep=${sweep_days:-off})"

# 1) Cross-compile / compat caches: not part of the local dev inner loop. They
#    are regenerated on demand by release/compat scripts.
for d in "$target_dir"/*-apple-darwin "$target_dir"/*-pc-windows-* "$target_dir"/linux-compat; do
  [[ -d "$d" && "$sweep_only" != "true" ]] || continue
  remove_path "$d" "cross-compile/compat cache"
done

# 2) Sweep stale artifact generations. When a crate is recompiled (feature or
#    flag change, dependency bump), cargo writes a new `name-<hash>` artifact
#    into deps/ and leaves the old one behind forever. This keeps the newest
#    generation per (crate, extension) so the warm cache is preserved, and only
#    deletes older generations whose mtime exceeds the --sweep day threshold.
#    Cargo transparently rebuilds anything it still needs, so this is safe.
# Bash 3 (macOS) has no associative arrays, and BSD sort has no portable -z.
# Keep paths in an indexed array, sorting only numeric metadata and hex keys.
# Emit NUL-delimited records so spaces, tabs and newlines survive end to end.
list_stale_generations() {
  local dir="$1" days="$2" kind="$3" path file key ext metadata index bytes
  local paths=() records=()
  path_is_contained "$dir" || return 0
  while IFS= read -r -d '' path; do
    metadata=$(stat "${stat_fmt[@]}" "$path" 2>/dev/null) || continue
    file=${path##*/}
    key="$file"
    if [[ "$kind" == f ]]; then
      ext=""
      if [[ "$file" =~ \.[A-Za-z0-9]+$ ]]; then
        ext="${BASH_REMATCH[0]}"
        key=${file%"$ext"}
      fi
      if [[ "$key" =~ -[0-9a-f]{16,}$ ]]; then
        key=${key%"${BASH_REMATCH[0]}"}
      fi
      key="$key$ext"
    else
      key=${key%-*}
    fi
    key=$(printf '%s' "$key" | od -An -v -tx1 | tr -d ' \n')
    index=${#paths[@]}
    paths[$index]="$path"
    records[$index]="$metadata $index $key"
  done < <(find "$dir" -mindepth 1 -maxdepth 1 -type "$kind" -print0 2>/dev/null)
  [[ ${#paths[@]} -gt 0 ]] || return 0
  while read -r index bytes; do
    if [[ "$kind" == f ]]; then printf '%s\0' "$bytes"; fi
    printf '%s\0' "${paths[$index]}"
  done < <(printf '%s\n' "${records[@]}" | sort -rn | awk -v cutoff="$(cutoff_epoch "$days")" '
    !($4 in seen) { seen[$4]=1; next }
    $1 < cutoff { print $3, $2 }')
}

if [[ -n "$sweep_days" ]]; then
  for profile_dir in "$target_dir"/debug "$target_dir"/release "$target_dir"/release-lto "$target_dir"/selfdev; do
    [[ -d "$profile_dir" ]] || continue
    path_is_contained "$profile_dir" || continue
    if path_has_active_process "$profile_dir"; then
      log "SKIP sweep (active process): $profile_dir"
      continue
    fi
    profile=$(basename "$profile_dir")
    if [[ -d "$profile_dir/deps" ]]; then
      swept=0
      swept_count=0
      while IFS= read -r -d '' bytes && IFS= read -r -d '' path; do
        [[ -n "$path" ]] || continue
        path_is_contained "$path" || continue
        if [[ "$apply" == "true" ]]; then
          rm -f -- "$path" || continue
        fi
        swept=$((swept + bytes))
        swept_count=$((swept_count + 1))
      done < <(list_stale_generations "$profile_dir/deps" "$sweep_days" f)
      verb=$([ "$apply" == true ] && echo "swept" || echo "would sweep")
      log "$verb $swept_count stale dep generations (>${sweep_days}d, kept newest per crate) from $profile  [$(human "$swept")]"
      total_reclaimed=$((total_reclaimed + swept))
    fi
    # Incremental compilation session dirs are cheap to regenerate and only
    # useful while their working set is hot; drop ones idle past the threshold.
    if [[ -d "$profile_dir/incremental" ]]; then
      while IFS= read -r -d '' d; do
        remove_path "$d" "stale incremental session (>${sweep_days}d)"
      done < <(list_stale_generations "$profile_dir/incremental" "$sweep_days" d)
    fi
  done
fi

# 3) Aggressive: cargo clean on stale (not-recently-active, no active process)
#    profiles to drop accumulated fingerprints/old artifact generations.
if [[ "$aggressive" == "true" ]]; then
  for profile_dir in "$target_dir"/debug "$target_dir"/release "$target_dir"/release-lto "$target_dir"/selfdev; do
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
