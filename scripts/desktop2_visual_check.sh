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

# Docs cannot rot: every test named in the checklist must actually exist.
echo "== checklist references real tests"
if ! python3 - "$crate" docs/DESKTOP2_VISUAL_CHECKLIST.md <<'PYEOF'
import pathlib, re, sys

crate, doc_path = sys.argv[1], sys.argv[2]
doc = pathlib.Path(doc_path).read_text()
src = "\n".join(p.read_text() for p in pathlib.Path(crate, "src").rglob("*.rs"))
defined = set(re.findall(r"fn ([a-z_][a-z0-9_]*)\(", src))
modules = {p.stem for p in pathlib.Path(crate, "src").rglob("*.rs")} | {
    "tests", "visual_tests", "action_tests", "selection_tests"
}
# Only audit the "Enforced by" column of the rule tables: those cells are the
# claim that a rule is machine-checked, so a name there must resolve to a real
# test. Prose and file references elsewhere are not claims.
referenced, missing = set(), []
for line in doc.splitlines():
    if not line.startswith("|") or "Enforced by" in line or set(line) <= set("|- "):
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if len(cells) < 3:
        continue
    for cell in re.findall(r"`([a-z_][a-z0-9_:.]*)`", cells[-1]):
        if cell.endswith(".rs"):
            continue
        leaf = cell.split("::")[-1]
        if leaf in modules:
            continue
        referenced.add(leaf)
        if leaf not in defined:
            missing.append(leaf)
if missing:
    print("checklist names tests that do not exist:", ", ".join(sorted(set(missing))))
    raise SystemExit(1)
print(f"  {len(referenced)} checklist test references all resolve")
PYEOF
then
  fail "checklist references a test that does not exist"
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
