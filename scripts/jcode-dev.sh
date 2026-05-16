#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

sync_mode="${JCODE_DEV_SYNC:-auto}"
passthrough_args=()
for arg in "$@"; do
  case "$arg" in
    --no-sync)
      sync_mode="skip"
      ;;
    --help-sync)
      cat <<'USAGE'
Usage: jcode-dev [--no-sync] [jcode args...]

Options:
  --no-sync          Skip git fetch/rebase preflight before launching jcode.

Environment:
  JCODE_DEV_SYNC=skip  Same as --no-sync.
  JCODE_DEV_SYNC=require
                     Fail instead of continuing when sync is blocked by local
                     changes.
  JCODE_DEV_USE_SELFDEV=1
                     Delegate to `selfdev enter` when a selfdev binary is
                     available. By default jcode-dev uses the local dev cargo
                     path to avoid slow release builds.
USAGE
      exit 0
      ;;
    *)
      passthrough_args+=("$arg")
      ;;
  esac
done
set -- "${passthrough_args[@]}"

log() {
  printf 'jcode-dev: %s\n' "$*" >&2
}

remote_exists() {
  git remote get-url "$1" >/dev/null 2>&1
}

is_worktree_clean() {
  [[ -z "$(git status --porcelain)" ]]
}

list_dirty_files() {
  git status --short | sed 's/^/  /' >&2
}

git_divergence() {
  local lhs="$1" rhs="$2"
  git rev-list --left-right --count "$lhs...$rhs" 2>/dev/null || printf '0\t0'
}

maybe_sync_branch() {
  local branch
  branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
  if [[ -z "$branch" ]]; then
    log "detached HEAD; skipping git sync preflight"
    return 0
  fi

  remote_exists upstream || {
    log "no upstream remote; skipping git sync preflight"
    return 0
  }

  local upstream_ref="upstream/$branch"
  if ! git show-ref --verify --quiet "refs/remotes/$upstream_ref"; then
    log "no $upstream_ref; skipping git sync preflight"
    return 0
  fi

  local ahead behind
  read -r ahead behind < <(git_divergence "$branch" "$upstream_ref")
  if (( behind == 0 )); then
    return 0
  fi

  log "$branch is $ahead ahead and $behind behind $upstream_ref"

  if ! is_worktree_clean; then
    log "working tree has local changes; not auto-rebasing"
    log "dirty files:"
    list_dirty_files
    log "refs: local=$branch upstream=$upstream_ref"
    log "continuing without sync to keep local work safe"
    log "to skip this preflight next time, run: jcode-dev --no-sync $*"
    log "to sync first, protect changes then run: git stash push -m 'wip before jcode-dev sync' && scripts/jcode-dev.sh"
    if [[ "${sync_mode}" == "require" ]]; then
      log "JCODE_DEV_SYNC=require set; stopping because sync is blocked"
      return 1
    fi
    return 0
  fi

  local backup
  backup="backup/${branch}-before-jcode-dev-sync-$(date -u +%Y%m%d-%H%M%S)"
  git branch "$backup" "$branch"
  log "created backup branch $backup"

  git rebase "$upstream_ref"

  if remote_exists origin && [[ "${JCODE_DEV_SYNC_PUSH:-0}" == "1" ]]; then
    local origin_ref="origin/$branch"
    git fetch origin --prune
    if git show-ref --verify --quiet "refs/remotes/$origin_ref"; then
      local origin_ahead origin_behind
      read -r origin_ahead origin_behind < <(git_divergence "$branch" "$origin_ref")
      if (( origin_ahead > 0 || origin_behind > 0 )); then
        log "updating origin/$branch with --force-with-lease"
        git push --force-with-lease origin "$branch"
      fi
    fi
  elif remote_exists origin; then
    local origin_ref="origin/$branch"
    if git show-ref --verify --quiet "refs/remotes/$origin_ref"; then
      local origin_ahead origin_behind
      read -r origin_ahead origin_behind < <(git_divergence "$branch" "$origin_ref")
      if (( origin_ahead > 0 || origin_behind > 0 )); then
        log "origin/$branch differs from local; set JCODE_DEV_SYNC_PUSH=1 to update fork with --force-with-lease"
      fi
    fi
  fi
}

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  case "$sync_mode" in
    skip|0|false|no|off)
      log "skipping git sync preflight"
      ;;
    auto|require|"")
      log "fetching remotes"
      git fetch --all --prune
      maybe_sync_branch "$@"
      ;;
    *)
      log "unsupported JCODE_DEV_SYNC=$sync_mode (expected auto, require, or skip)"
      exit 1
      ;;
  esac
fi

if [[ "${JCODE_DEV_USE_SELFDEV:-0}" == "1" ]] && command -v selfdev >/dev/null 2>&1; then
  exec selfdev enter "$@"
fi

profile="${JCODE_DEV_CARGO_PROFILE:-dev}"
profile_args=()
if [[ "$profile" != "dev" ]]; then
  profile_args=(--profile "$profile")
fi

export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

exec "$repo_root/scripts/dev_cargo.sh" run "${profile_args[@]}" -p jcode --bin jcode -- "$@"
