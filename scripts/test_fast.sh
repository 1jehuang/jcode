#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cargo_exec="$repo_root/scripts/cargo_exec.sh"

run_cargo() {
  (cd "$repo_root" && "$cargo_exec" "$@")
}

echo "=== Fast test loop (lib + bins) ==="
run_cargo test --lib --bins "$@"

echo ""
echo "For full coverage, run: scripts/test_e2e.sh"
