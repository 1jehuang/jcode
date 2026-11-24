#!/usr/bin/env bash
# Self-check for clean_stale_target.sh and the dev_cargo.sh free-space guard.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
mkdir -p "$t/debug/incremental/x" "$t/debug/deps"
touch "$t/debug/deps/fresh.rlib"
touch -t 202001010000 "$t/debug/deps/old.rlib"

out=$("$here/clean_stale_target.sh" --dry-run "$t")
grep -q "incremental" <<<"$out"; grep -q "old.rlib" <<<"$out"
if grep -q "fresh.rlib" <<<"$out"; then echo "FAIL: fresh artifact listed" >&2; exit 1; fi
[[ -d "$t/debug/incremental" ]]  # dry run deletes nothing

if ! pgrep -x cargo >/dev/null; then
  "$here/clean_stale_target.sh" "$t" >/dev/null
  [[ ! -e "$t/debug/incremental" && ! -e "$t/debug/deps/old.rlib" && -e "$t/debug/deps/fresh.rlib" ]]
fi

# Guard: an impossible threshold must refuse before invoking cargo.
if JCODE_MIN_FREE_GB=999999 "$here/dev_cargo.sh" --version 2>"$t/err"; then
  echo "FAIL: guard did not refuse" >&2; exit 1
fi
grep -q "refusing to build" "$t/err"
echo "PASS"
