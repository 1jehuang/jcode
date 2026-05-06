#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
strict_swallowed=0
run_check=0

usage() {
  cat <<'USAGE'
Usage:
  scripts/cli_quality_preflight.sh [--strict-swallowed] [--check]

Runs a bounded CLI/TUI quality gate:
  1) rustfmt check, or cargo fmt when --check is omitted
  2) panic-prone budget
  3) swallowed-error budget report, warning by default because the repo has known debt
  4) dependency boundary guard
  5) jcode-tui-style unit tests
  6) cargo check -p jcode

Options:
  --strict-swallowed  Fail when check_swallowed_error_budget.py fails
  --check             Do not modify formatting, use cargo fmt --check
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --strict-swallowed)
      strict_swallowed=1
      ;;
    --check)
      run_check=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

cd "$repo_root"

cargo_cmd=(cargo)
if [[ -x "$repo_root/scripts/cargo_exec.sh" ]]; then
  cargo_cmd=("$repo_root/scripts/cargo_exec.sh")
fi

step() {
  echo
  echo "=== $* ==="
}

step "CLI quality preflight: formatting"
if [[ "$run_check" -eq 1 ]]; then
  "${cargo_cmd[@]}" fmt --check
else
  "${cargo_cmd[@]}" fmt
fi

step "CLI quality preflight: panic budget"
python3 scripts/check_panic_budget.py

step "CLI quality preflight: swallowed-error budget"
set +e
python3 scripts/check_swallowed_error_budget.py
swallowed_status=$?
set -e
if [[ "$swallowed_status" -ne 0 ]]; then
  if [[ "$strict_swallowed" -eq 1 ]]; then
    echo "error: swallowed-error budget failed in strict mode" >&2
    exit "$swallowed_status"
  fi
  echo "warning: swallowed-error budget currently fails; continuing in non-strict mode" >&2
  echo "         use --strict-swallowed once baseline debt is reduced or intentionally ratcheted" >&2
fi

step "CLI quality preflight: dependency boundaries"
python3 scripts/check_dependency_boundaries.py

step "CLI quality preflight: jcode-tui-style tests"
"${cargo_cmd[@]}" test -p jcode-tui-style

step "CLI quality preflight: jcode check"
"${cargo_cmd[@]}" check -p jcode

step "CLI quality preflight passed"
if [[ "$swallowed_status" -ne 0 ]]; then
  echo "Passed with non-strict swallowed-error warning."
else
  echo "All checks passed."
fi
