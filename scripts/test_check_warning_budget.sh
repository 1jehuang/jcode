#!/usr/bin/env bash
# Contract tests for scripts/check_warning_budget.sh using a fake cargo.
# Runs without compiling anything. Usage: scripts/test_check_warning_budget.sh
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
gate="$repo_root/scripts/check_warning_budget.sh"
baseline=$(tr -d '[:space:]' < "$repo_root/scripts/warning_budget.txt")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# fake cargo: prints $FAKE_OUTPUT to stderr, exits $FAKE_STATUS
cat > "$tmp/cargo" <<'FAKE'
#!/usr/bin/env bash
printf '%b' "${FAKE_OUTPUT:-}" >&2
exit "${FAKE_STATUS:-0}"
FAKE
chmod +x "$tmp/cargo"

failures=0
run_case() {
  local name=$1 expect_status=$2 expect_text=$3 output=$4 status=$5
  local actual_status=0 actual_out
  actual_out=$(CARGO="$tmp/cargo" FAKE_OUTPUT="$output" FAKE_STATUS="$status" bash "$gate" 2>&1) \
    || actual_status=$?
  if [[ "$actual_status" != "$expect_status" ]] || [[ "$actual_out" != *"$expect_text"* ]]; then
    echo "FAIL: $name (exit $actual_status, want $expect_status)"
    printf '%s\n' "$actual_out" | sed 's/^/    /'
    failures=$((failures + 1))
  else
    echo "ok: $name"
  fi
}

warnings() {
  local n=$1 out=""
  for ((i = 0; i < n; i++)); do out+="warning: fake warning $i\n"; done
  printf '%s' "$out"
}

run_case "clean success" 0 "current=0" "" 0
run_case "warnings at budget" 0 "current=$baseline" "$(warnings "$baseline")" 0
run_case "warnings above budget show diagnostics" 1 "warning: fake warning $baseline" \
  "$(warnings $((baseline + 1)))" 0
run_case "compiler failure without warnings" 101 "deliberate compiler failure" \
  "error: deliberate compiler failure\n" 101
run_case "compiler failure with warnings" 101 "cargo check failed" \
  "warning: w\nerror: boom\n" 101
if (( baseline > 0 )); then
  run_case "warnings below budget" 0 "improved" "$(warnings $((baseline - 1)))" 0
fi

missing_status=0
CARGO="$tmp/does-not-exist" bash "$gate" > /dev/null 2>&1 || missing_status=$?
if [[ "$missing_status" == 1 ]]; then echo "ok: missing cargo"; else
  echo "FAIL: missing cargo (exit $missing_status)"; failures=$((failures + 1)); fi

if (( failures > 0 )); then
  echo "$failures warning-budget contract case(s) failed" >&2
  exit 1
fi
echo "all warning-budget contract cases passed"
