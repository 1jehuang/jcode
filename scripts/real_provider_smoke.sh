#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
provider=${JCODE_PROVIDER:-auto}
prompt=${1:-"Use the bash tool to run 'pwd', then use the ls tool to list the current directory, then respond with DONE."}
expect=${JCODE_TRACE_EXPECT:-DONE}

echo "=== Real Provider Smoke ==="
echo "Provider: ${provider}"

if [[ "${JCODE_REAL_PROVIDER_TEST_API:-1}" == "1" ]]; then
  if [[ "${provider}" == "claude" && "${JCODE_USE_DIRECT_API:-0}" != "1" ]]; then
    echo ""
    echo "Test 1: Claude CLI smoke (test_api)"
    (cd "$repo_root" && cargo run --bin test_api)
  else
    echo ""
    echo "Test 1: Skipping test_api (provider=${provider}, JCODE_USE_DIRECT_API=${JCODE_USE_DIRECT_API:-0})"
  fi
fi

echo ""
echo "Test 2: Tool harness (network tools enabled)"
(cd "$repo_root" && cargo run --bin jcode-harness -- --include-network)

echo ""
echo "Test 3: End-to-end trace"
if [[ ! -x "$repo_root/target/release/jcode" ]]; then
  (cd "$repo_root" && cargo build --release)
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

set +e
output=$(JCODE_HOME="$workdir" PATH="$repo_root/target/release:$PATH" \
  jcode run --no-update --trace --provider "$provider" "$prompt" 2>&1)
status=$?
set -e

printf "%s\n" "$output"

if [[ $status -ne 0 ]]; then
  echo "Trace failed with exit code $status" >&2
  exit $status
fi

if [[ -n "$expect" ]] && ! grep -q "$expect" <<<"$output"; then
  echo "Trace output did not include expected marker: ${expect}" >&2
  exit 1
fi

echo ""
echo "=== Real provider smoke OK ==="
