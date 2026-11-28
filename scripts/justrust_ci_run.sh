#!/usr/bin/env bash
# Run a Cargo command via hosted justrust in CI when JCODE_API_KEY is available,
# falling back to local cargo on fork PRs or when hosted justrust is disabled.
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: justrust_ci_run.sh <cargo-subcommand> [args...]" >&2
  exit 1
fi

subcmd="$1"
shift

if [[ "${JUSTRUST_CI_DISABLE:-0}" != "1" ]] && [[ -n "${JCODE_API_KEY:-}" ]] && command -v justrust >/dev/null 2>&1; then
  exec justrust "$subcmd" --remote "$@"
fi

exec cargo "$subcmd" "$@"
