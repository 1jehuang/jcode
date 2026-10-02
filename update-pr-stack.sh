#!/usr/bin/env bash
# Rebuild local/pr-stack on top of latest upstream + PR branches, install side-by-side.
# Never touches stable/current/launcher — those stay on the official release.
# Usage: ./update-pr-stack.sh [--fast]
#   --fast: plain release profile (quicker, larger binary). Default: release-lto.
set -euo pipefail
cd "$(dirname "$0")"

echo "=== fetch ==="
git fetch origin
git fetch fork

echo "=== recreate local/pr-stack off origin/master ==="
# STACK.toml + tooling live on local/pr-stack, not origin/master: snapshot
# them BEFORE the destructive recreate, then restore after.
TOOLING_TMP=$(mktemp -d)
for f in STACK.toml STACK.md gen-stack-md.sh update-pr-stack.sh; do
  [ -f "$f" ] && cp "$f" "$TOOLING_TMP/" || true
done
git checkout -q master 2>/dev/null || git checkout -q -b master origin/master
git branch -D local/pr-stack 2>/dev/null || true
git checkout -q -b local/pr-stack origin/master
for f in STACK.toml STACK.md gen-stack-md.sh update-pr-stack.sh; do
  [ -f "$TOOLING_TMP/$f" ] && cp "$TOOLING_TMP/$f" ./ || true
done
rm -rf "$TOOLING_TMP"
chmod +x gen-stack-md.sh update-pr-stack.sh 2>/dev/null || true

echo "=== branch list from STACK.toml (state=in-stack, in file order) ==="
BRANCHES=$(python3 -c "
import re
txt = open('STACK.toml').read()
names, states = [], {}
for m in re.finditer(r'\[\[branch\]\]\nname = \"([^\"]+)\"\nstate = \"([^\"]+)\"', txt):
    names.append(m.group(1)); states[m.group(1)] = m.group(2)
print(' '.join(b for b in names if states[b] == 'in-stack'))
")
echo "merging: $BRANCHES"
for b in $BRANCHES; do
  # NOTE: nested branches (tunable-dedup-rrf, fix/tui-agentsmd-working-dir)
  # ride inside other branches and are state=nested in STACK.toml, so they
  # never merge directly. Parked branches are state=parked. To add a branch,
  # edit STACK.toml — never this list.
  echo "=== merge $b ==="
  if ! git merge --no-edit "$b"; then
    echo "CONFLICT in $b — resolve manually, commit, then run ./update-pr-stack.sh --resume-build."
    echo "Or to skip the rebuild: cargo build --release -p jcode --bin jcode"
    exit 1
  fi
done

if [[ "${1:-}" == "--resume-build" ]]; then
  echo "=== resuming from existing resolution ==="
elif git grep -l "<<<<<<<" -- . | head -3; then
  echo "Leftover conflict markers. Resolve, commit, re-run with --resume-build."
  exit 1
fi

echo "=== check ==="
if cargo check --workspace 2>&1 | grep -E "^error"; then
  echo "CHECK FAILED"
  exit 1
fi
echo "check clean"

echo "=== build (embeddings feature REQUIRED ==="
echo "Without it memory import stores no vectors and the harness scores"
echo "uniform 0.0 (2026-09-30 red-gate lesson). ==="
if [[ "${1:-}" == "--fast" ]]; then
  cargo build --release --features embeddings -p jcode --bin jcode
  BIN=target/release/jcode
else
  cargo build --profile release-lto --features embeddings -p jcode --bin jcode
  BIN=target/release-lto/jcode
fi

# Sanity: the installed binary must carry the real embedder, not the stub.
# Without it every import stores vector-less rows and C1/C2/C4 score 0.0.
if strings "$BIN" 2>/dev/null | grep -q "Embeddings feature not compiled"; then
  echo "BUILD-BUG: $BIN carries the embedding stub (feature off?) — refusing to install" >&2
  exit 1
fi

echo "=== versioned side-by-side install (stable untouched) ==="
HASH=$(git rev-parse --short=12 HEAD)
DEST=~/.jcode/builds/versions/pr-stack-$HASH
mkdir -p "$DEST"
install -m 755 "$BIN" "$DEST/jcode"
ln -sfn "$DEST/jcode" ~/.jcode/builds/pr-stack
echo "Installed: $DEST/jcode"
echo "Symlink:   ~/.jcode/builds/pr-stack/jcode"
echo ""
echo "Run it:    ~/.jcode/builds/pr-stack/jcode"
echo "Make it your launcher (reversible):"
echo "  ln -sfn ~/.jcode/builds/pr-stack/jcode ~/.local/bin/jcode"
echo "Back to stable:"
echo "  ln -sfn ~/.jcode/builds/current/jcode ~/.local/bin/jcode   # current still points at stable"
