#!/usr/bin/env bash
# gen-stack-md.sh — regenerate STACK.md from STACK.toml + live git/gh state.
# Usage: ./gen-stack-md.sh
# Read-only except STACK.md. Never hand-edit STACK.md; edit STACK.toml.
set -euo pipefail
cd "$(dirname "$0")"

STACK_HEAD=$(git rev-parse --short HEAD)
STACK_FULL=$(git rev-parse HEAD)
BINARY_VER=$(~/.jcode/builds/pr-stack --version 2>/dev/null || echo "binary missing")
BINARY_HASH=$(echo "$BINARY_VER" | grep -oE '\([0-9a-f]{7,40}\)' | tr -d '()' || true)
# Code-aware freshness: docs/scripts-only commits (STACK.md snapshots, toml
# notes) move HEAD without changing the binary. Compare code trees, not hashes.
FRESHNESS="binary version unparsed"
if [ -n "$BINARY_HASH" ]; then
  if CODE_DELTA=$(git diff "$BINARY_HASH"..HEAD --stat -- crates src Cargo.toml Cargo.lock 2>/dev/null); then
    if [ -z "$CODE_DELTA" ]; then
      FRESHNESS="binary matches stack (docs-only drift since build)"
    else
      FRESHNESS="STALE: code changed since binary $BINARY_HASH — rebuild"
    fi
  else
    FRESHNESS="STALE: binary $BINARY_HASH not in history — rebuild"
  fi
fi

{
echo "# STACK — local jcode stack (generated $(date -u +%Y-%m-%d), do not hand-edit)"
echo ""
echo "Source: \`STACK.toml\`. Regenerate: \`./gen-stack-md.sh\`."
echo "Policy: build locally, publish once. No new PRs until stack declared done."
echo "Exception: genuine fixes to upstream bugs may file anytime."
echo ""
echo "Stack HEAD: \`$STACK_HEAD\` (\`$STACK_FULL\`)"
echo "Binary: \`$BINARY_VER\` — $FRESHNESS"
echo ""
echo "## Branches"
echo ""
echo "| Branch | State | Tip | PR | Note |"
echo "|--------|-------|-----|----|------|"
python3 - <<'EOF'
import re, subprocess
txt = open('STACK.toml').read()
tips = {}
try:
    out = subprocess.run(['git', 'rev-parse', '--short'] + [], capture_output=True, text=True)
except Exception:
    pass
for m in re.finditer(r'\[\[branch\]\]\nname = "([^"]+)"\nstate = "([^"]+)"\npr = (\d+)(?:\nnote = "([^"]*)")?', txt):
    name = m.group(1)
    try:
        tip = subprocess.run(['git', 'rev-parse', '--short', name], capture_output=True, text=True).stdout.strip()
    except Exception:
        tip = '?'
    print(f"| {name} | {m.group(2)} | {tip} | {m.group(3) if m.group(3)!='0' else '-'} | {m.group(4) or ''} |")
EOF
echo ""
echo "## Live verification"
echo ""
echo '```'
git log --first-parent --format='%h %s' origin/master..local/pr-stack 2>/dev/null | head -20 || echo "local/pr-stack not on this base"
echo '```'
} > STACK.md
echo "Wrote STACK.md at stack $STACK_HEAD"
