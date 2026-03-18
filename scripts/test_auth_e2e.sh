#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
provider=${JCODE_PROVIDER:-auto}
prompt=${JCODE_AUTH_TEST_PROMPT:-"Reply with exactly AUTH_TEST_OK and nothing else. Do not call tools."}

echo "=== Auth E2E Test ==="
echo "Provider: ${provider}"

args=(auth-test --prompt "$prompt")

if [[ "${provider}" != "auto" ]]; then
  args=(--provider "$provider" "${args[@]}")
else
  args+=(--all-configured)
fi

if [[ "${JCODE_AUTH_TEST_LOGIN:-0}" == "1" ]]; then
  args+=(--login)
fi

if [[ "${JCODE_AUTH_TEST_NO_SMOKE:-0}" == "1" ]]; then
  args+=(--no-smoke)
fi

if [[ "${JCODE_AUTH_TEST_JSON:-0}" == "1" ]]; then
  args+=(--json)
fi

(cd "$repo_root" && cargo run --bin jcode -- "${args[@]}")

echo ""
echo "=== Auth E2E OK ==="
