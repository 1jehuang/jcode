#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
baseline_file="$repo_root/scripts/warning_budget.txt"

usage() {
  cat <<'USAGE'
Usage:
  scripts/check_warning_budget.sh            # fail if warnings exceed baseline
  scripts/check_warning_budget.sh --update   # update baseline to current warning count

Notes:
  - Counts Rust compiler lines that begin with "warning:" from `cargo check -q`
  - Baseline is stored in scripts/warning_budget.txt
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ ! -f "$baseline_file" ]]; then
  echo "error: missing baseline file: $baseline_file" >&2
  exit 1
fi

# Use grep, not rg: ripgrep is not installed on the CI runner, and the
# `|| printf 0` fallback turned "rg: command not found" into "zero warnings",
# so this gate passed vacuously in CI for as long as it has existed. grep is
# guaranteed present, and `grep -c` exits 1 on no matches, which the fallback
# still handles correctly.
if ! command -v cargo > /dev/null 2>&1; then
  echo "error: cargo not found" >&2
  exit 1
fi
current=$(cd "$repo_root" && CARGO_TERM_COLOR=never cargo check -q 2>&1 | grep -c '^warning:' || true)
current=$(printf '%s' "${current:-0}" | tr -d '[:space:]')
baseline=$(tr -d '[:space:]' < "$baseline_file")

if [[ "${1:-}" == "--update" ]]; then
  printf '%s\n' "$current" > "$baseline_file"
  echo "Updated warning baseline: $baseline"
  echo "New warning baseline: $current"
  exit 0
fi

if ! [[ "$baseline" =~ ^[0-9]+$ ]]; then
  echo "error: invalid warning baseline in $baseline_file: '$baseline'" >&2
  exit 1
fi

if (( current > baseline )); then
  echo "Warning budget exceeded: current=$current baseline=$baseline" >&2
  echo "Run scripts/check_warning_budget.sh --update only after intentional cleanup." >&2
  exit 1
fi

if (( current < baseline )); then
  echo "Warning budget improved: current=$current baseline=$baseline"
  echo "Consider running: scripts/check_warning_budget.sh --update"
else
  echo "Warning budget OK: current=$current baseline=$baseline"
fi
