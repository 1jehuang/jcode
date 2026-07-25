#!/usr/bin/env bash
# Enforce the mechanical rules from docs/DESKTOP2_VISUAL_CHECKLIST.md that are
# about source shape rather than rendered output.
#
#   scripts/desktop2_visual_check.sh          # lint + fast tests
#   scripts/desktop2_visual_check.sh --gpu    # also run pixel-level tests
set -euo pipefail

cd "$(dirname "$0")/.."
crate=crates/jcode-desktop2
status=0

fail() {
  echo "FAIL: $1" >&2
  status=1
}

# 4.1 Scene code speaks semantic theme roles, never literal colors. Literal
# colors are allowed only in theme.rs (where themes are defined) and in tests.
literals=$(grep -n "Color::from_rgb8\|Color::WHITE\|Color::BLACK" \
  "$crate/src/main.rs" "$crate/src/layout.rs" 2>/dev/null || true)
if [ -n "$literals" ]; then
  fail "literal colors in scene code (use theme roles):"
  echo "$literals" >&2
fi

# 1.1 Layout geometry belongs in layout.rs, so it stays testable. Scene code
# must not invent its own measure/gutter/spacing constants.
geometry=$(grep -nE '^\s*const (MEASURE|GUTTER|MARGIN|COLUMN|SPACE|PAD)[A-Z_]*' \
  "$crate/src/main.rs" 2>/dev/null || true)
if [ -n "$geometry" ]; then
  fail "layout geometry declared outside layout.rs:"
  echo "$geometry" >&2
fi

# 3.1 One font family, declared once, in text.rs only.
if [ "$(grep -c 'JetBrains Mono' "$crate/src/text.rs" | tr -d ' ')" -lt 1 ]; then
  fail "text.rs must declare the JetBrains Mono font stack"
fi
stray=$(grep -rln 'JetBrains Mono' "$crate/src" | grep -v 'text.rs' || true)
if [ -n "$stray" ]; then
  fail "font family referenced outside text.rs: $stray"
fi

echo "== fast invariants (geometry, typography, theme)"
cargo test --profile selfdev -p jcode-desktop2 --quiet || status=1

if [ "${1:-}" = "--gpu" ]; then
  echo "== pixel-level visual invariants"
  cargo test --profile selfdev -p jcode-desktop2 --quiet -- --ignored || status=1
fi

if [ "$status" -eq 0 ]; then
  echo "desktop2 visual checklist: OK"
fi
exit "$status"
