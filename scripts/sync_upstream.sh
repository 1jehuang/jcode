#!/usr/bin/env bash
# sync_upstream.sh
# -----------------------------------------------------------------------------
# Pull 1jehuang/jcode's latest (origin/master) and replay YOUR local fix commits
# on top of it, then rebuild + install + push to your fork.
#
# Why this exists:
#   `jcode update` runs `git pull --ff-only` against your branch's upstream
#   (your fork). With your own commits on top, you can never fast-forward to
#   1jehuang's master directly. This script does the safe rebase for you so you
#   get upstream updates *timely* while keeping your fixes.
#
# Usage:
#   scripts/sync_upstream.sh            # rebase onto origin/master, build, install
#   scripts/sync_upstream.sh --no-build # just rebase + push, skip build/install
#
# It is safe: it tags a backup before rewriting history and aborts a rebase on
# conflict instead of leaving you in a broken state.
# -----------------------------------------------------------------------------
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$repo_root"

UPSTREAM_REMOTE="${JCODE_UPSTREAM_REMOTE:-origin}"     # 1jehuang/jcode
UPSTREAM_BRANCH="${JCODE_UPSTREAM_BRANCH:-master}"
FORK_REMOTE="${JCODE_FORK_REMOTE:-fork}"               # your fork
do_build=1
[[ "${1:-}" == "--no-build" ]] && do_build=0

branch="$(git rev-parse --abbrev-ref HEAD)"
echo "==> Current branch: $branch"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "ERROR: working tree is dirty. Commit or stash first." >&2
  git status --short >&2
  exit 1
fi

backup_tag="sync-backup-$(date +%Y%m%d%H%M%S)"
git tag -f "$backup_tag" HEAD >/dev/null
echo "==> Backup tag: $backup_tag -> $(git rev-parse --short HEAD)"

echo "==> Fetching $UPSTREAM_REMOTE ($UPSTREAM_BRANCH) ..."
git fetch "$UPSTREAM_REMOTE" "$UPSTREAM_BRANCH" --quiet

ahead="$(git rev-list --count "$UPSTREAM_REMOTE/$UPSTREAM_BRANCH"..HEAD)"
behind="$(git rev-list --count HEAD.."$UPSTREAM_REMOTE/$UPSTREAM_BRANCH")"
echo "==> You have $ahead local commit(s); upstream is $behind commit(s) ahead."

if [[ "$behind" -eq 0 ]]; then
  echo "==> Already up to date with $UPSTREAM_REMOTE/$UPSTREAM_BRANCH. Nothing to rebase."
else
  echo "==> Rebasing your $ahead commit(s) onto $UPSTREAM_REMOTE/$UPSTREAM_BRANCH ..."
  if ! git rebase "$UPSTREAM_REMOTE/$UPSTREAM_BRANCH"; then
    echo "ERROR: rebase hit conflicts. Aborting and restoring your previous state." >&2
    git rebase --abort || true
    echo "Your branch is unchanged. Resolve manually with:" >&2
    echo "  git rebase $UPSTREAM_REMOTE/$UPSTREAM_BRANCH" >&2
    echo "Backup is at tag: $backup_tag" >&2
    exit 1
  fi
fi

if [[ "$do_build" -eq 1 ]]; then
  echo "==> Building (release) ..."
  cargo build --release --bin jcode
  echo "==> Installing into ~/.jcode builds ..."
  JCODE_RELEASE_PROFILE=release bash "$repo_root/scripts/install_release.sh" --fast
fi

echo "==> Force-pushing rebased branch to $FORK_REMOTE/$branch ..."
git push "$FORK_REMOTE" "+HEAD:$branch"

echo ""
echo "Done. You now have $UPSTREAM_REMOTE/$UPSTREAM_BRANCH + your fixes on top."
echo "Backup of the pre-sync state is tag: $backup_tag"
echo "Restart jcode (pkill -f 'jcode.*serve' then launch) to run the new binary."
