#!/usr/bin/env bash
# Canonical local build + install for jcode on macOS: disk check, build, atomic replace, ad-hoc sign, verify.
set -euo pipefail

REPO="$HOME/jcode"
DEST="$HOME/.jcode/builds/current/jcode"
MIN_FREE_KB=$((15 * 1024 * 1024))

free_kb=$(df -k / | awk 'NR==2 {print $4}')
if [ "$free_kb" -lt "$MIN_FREE_KB" ]; then
  echo "ABORT: only $((free_kb / 1024 / 1024)) GB free on /, need 15 GB. Run jclean first." >&2
  exit 1
fi

cd "$REPO"
cargo build --release

mkdir -p "$(dirname "$DEST")"
# rm first so macOS never reuses a cached Mach-O signature for the old inode
rm -f "$DEST"
cp ./target/release/jcode "$DEST"
codesign -s - -f "$DEST"

git worktree prune
"$DEST" --version
