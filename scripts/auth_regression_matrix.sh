#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

bin=${KCODE_AUTH_MATRIX_BIN:-}
out_dir=${KCODE_AUTH_MATRIX_OUT:-"$repo_root/target/auth-test-reports"}
prompt=${KCODE_AUTH_MATRIX_PROMPT:-"Reply with exactly AUTH_TEST_OK and nothing else. Do not call tools."}
providers=${KCODE_AUTH_MATRIX_PROVIDERS:-"claude copilot openrouter deepseek zai alibaba-coding-plan openai-compatible"}
mode=${KCODE_AUTH_MATRIX_MODE:-configured}
keep_going=${KCODE_AUTH_MATRIX_KEEP_GOING:-1}
per_command_timeout=${KCODE_AUTH_MATRIX_TIMEOUT:-90}

usage() {
  cat <<'EOF'
Usage: scripts/auth_regression_matrix.sh [options]

Runs jcode auth-test across the auth/provider matrix and writes one JSON report per provider.
By default it only tests providers that are configured enough for auth-test to run.

Options:
  --all                 Try every provider in the matrix, even if not configured
  --configured          Test only configured providers (default)
  --provider NAME       Test one provider. Can be repeated.
  --out DIR             Report directory (default: target/auth-test-reports)
  --bin PATH            jcode binary to run (default: cargo run --bin jcode --)
  --login               Run login before validation for each provider
  --no-smoke            Skip runtime model smoke
  --no-tool-smoke       Skip tool-enabled runtime smoke
  --fail-fast           Stop after the first failed provider
  --prompt TEXT         Custom smoke prompt
  --timeout SECONDS     Per auth-test command timeout (default: 90)
  -h, --help            Show this help

Environment equivalents:
  KCODE_AUTH_MATRIX_BIN=/path/to/jcode
  KCODE_AUTH_MATRIX_OUT=target/auth-test-reports
  KCODE_AUTH_MATRIX_PROVIDERS="claude deepseek zai"
  KCODE_AUTH_MATRIX_MODE=configured|all
  KCODE_AUTH_MATRIX_LOGIN=1
  KCODE_AUTH_MATRIX_NO_SMOKE=1
  KCODE_AUTH_MATRIX_NO_TOOL_SMOKE=1
  KCODE_AUTH_MATRIX_KEEP_GOING=0
  KCODE_AUTH_MATRIX_TIMEOUT=90

Examples:
  scripts/auth_regression_matrix.sh --configured --no-smoke
  scripts/auth_regression_matrix.sh --provider deepseek --provider zai
  KCODE_AUTH_MATRIX_BIN=target/selfdev/jcode scripts/auth_regression_matrix.sh --all
EOF
}

selected=()
extra_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --all)
      mode=all
      shift
      ;;
    --configured)
      mode=configured
      shift
      ;;
    --provider)
      [[ $# -ge 2 ]] || { echo "error: --provider requires a value" >&2; exit 2; }
      selected+=("$2")
      shift 2
      ;;
    --out)
      [[ $# -ge 2 ]] || { echo "error: --out requires a value" >&2; exit 2; }
      out_dir=$2
      shift 2
      ;;
    --bin)
      [[ $# -ge 2 ]] || { echo "error: --bin requires a value" >&2; exit 2; }
      bin=$2
      shift 2
      ;;
    --login)
      extra_args+=(--login)
      shift
      ;;
    --no-smoke)
      extra_args+=(--no-smoke)
      shift
      ;;
    --no-tool-smoke)
      extra_args+=(--no-tool-smoke)
      shift
      ;;
    --fail-fast)
      keep_going=0
      shift
      ;;
    --prompt)
      [[ $# -ge 2 ]] || { echo "error: --prompt requires a value" >&2; exit 2; }
      prompt=$2
      shift 2
      ;;
    --timeout)
      [[ $# -ge 2 ]] || { echo "error: --timeout requires a value" >&2; exit 2; }
      per_command_timeout=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "${KCODE_AUTH_MATRIX_LOGIN:-0}" == "1" ]]; then
  extra_args+=(--login)
fi
if [[ "${KCODE_AUTH_MATRIX_NO_SMOKE:-0}" == "1" ]]; then
  extra_args+=(--no-smoke)
fi
if [[ "${KCODE_AUTH_MATRIX_NO_TOOL_SMOKE:-0}" == "1" ]]; then
  extra_args+=(--no-tool-smoke)
fi

if [[ ${#selected[@]} -eq 0 ]]; then
  # shellcheck disable=SC2206
  selected=($providers)
fi

mkdir -p "$out_dir"

run_jcode() {
  if [[ -n "$bin" ]]; then
    timeout "$per_command_timeout" "$bin" "$@"
  else
    timeout "$per_command_timeout" cargo run --quiet --bin jcode -- "$@"
  fi
}

configured_json="$out_dir/configured-providers.json"
if [[ "$mode" == "configured" ]]; then
  echo "Discovering configured providers..."
  rm -f "$configured_json"
  if ! run_jcode auth-test --all-configured --no-smoke --no-tool-smoke --json --output "$configured_json" >/tmp/jcode-auth-matrix-discovery.out 2>/tmp/jcode-auth-matrix-discovery.err; then
    if [[ -s "$configured_json" ]]; then
      echo "note: configured-provider discovery reported non-ready providers; continuing with per-provider classification" >&2
    else
      cat /tmp/jcode-auth-matrix-discovery.err >&2 || true
      echo "warning: configured-provider discovery failed; continuing with explicit matrix and skipping only obvious unconfigured failures" >&2
    fi
  fi
fi

failed=()
passed=()
skipped=()
blocked=()

is_unconfigured_failure() {
  grep -Eiq 'not configured|missing|no credentials|not found in environment|requires.*token|requires.*api key' "$1"
}

is_external_account_blocked_failure() {
  # These are upstream account/entitlement states, not auth-regression signal.
  # Keep this list intentionally narrow so real code/provider failures still fail.
  grep -Eiq 'feature_flag_blocked|can_signup_for_limited|Contact Support|not entitled|not eligible|subscription required|quota exceeded|rate limit' "$1"
}

echo "Auth regression matrix"
echo "Mode: $mode"
echo "Reports: $out_dir"
echo "Providers: ${selected[*]}"
echo "Timeout: ${per_command_timeout}s per command"
echo

for provider in "${selected[@]}"; do
  report="$out_dir/${provider}.json"
  log="$out_dir/${provider}.log"
  args=(auth-test --provider "$provider" --prompt "$prompt" --json --output "$report" "${extra_args[@]}")

  echo "=== auth-test: $provider ==="
  set +e
  run_jcode "${args[@]}" >"$log" 2>&1
  status=$?
  set -e

  if [[ $status -eq 0 ]]; then
    passed+=("$provider")
    echo "PASS $provider"
  else
    if [[ "$mode" == "configured" ]] && is_unconfigured_failure "$log"; then
      skipped+=("$provider")
      echo "SKIP $provider (not configured, see $log)"
    elif [[ "$mode" == "configured" ]] && is_external_account_blocked_failure "$log"; then
      blocked+=("$provider")
      echo "BLOCKED $provider (upstream account/entitlement unavailable, see $log)"
    else
      failed+=("$provider")
      echo "FAIL $provider (exit $status, see $log)"
      if [[ "$keep_going" != "1" ]]; then
        break
      fi
    fi
  fi
  echo
done

summary="$out_dir/summary.txt"
{
  echo "passed: ${passed[*]:-<none>}"
  echo "skipped: ${skipped[*]:-<none>}"
  echo "blocked: ${blocked[*]:-<none>}"
  echo "failed: ${failed[*]:-<none>}"
} | tee "$summary"

if [[ ${#failed[@]} -gt 0 ]]; then
  exit 1
fi
