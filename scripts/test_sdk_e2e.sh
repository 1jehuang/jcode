#!/usr/bin/env bash
# End-to-end check for the TypeScript SDK against a real harness bridge.
#
# Unit tests use a mock server, which cannot catch translation mismatches (the
# bridge answers `send_message` with a `message_accepted` event rather than a
# reply, and only a live run reveals that). This script builds the bridge,
# points it at the running daemon, and drives one real turn through the SDK.
#
# Usage: scripts/test_sdk_e2e.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

sdk_dir="$repo_root/sdk/typescript"
socket="${TMPDIR:-/tmp}/jcode-sdk-e2e-$$.sock"
log="${TMPDIR:-/tmp}/jcode-sdk-e2e-$$.log"

echo "== building SDK =="
npm --prefix "$sdk_dir" install --no-audit --no-fund --silent
npm --prefix "$sdk_dir" run build --silent

echo "== building bridge =="
cargo build --profile selfdev -p jcode-harness-api-server --bin jcode-harness-api-bridge

echo "== starting bridge on $socket =="
JCODE_API_SOCKET="$socket" ./target/selfdev/jcode-harness-api-bridge >"$log" 2>&1 &
bridge_pid=$!
cleanup() {
  kill "$bridge_pid" 2>/dev/null || true
  rm -f "$socket"
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  [ -S "$socket" ] && break
  sleep 0.1
done
if [ ! -S "$socket" ]; then
  echo "bridge failed to start:"; cat "$log"; exit 1
fi

echo "== driving a real turn =="
JCODE_API_SOCKET="$socket" node "$sdk_dir/test/live-turn.mjs"

echo "== exercising the control surface =="
JCODE_API_SOCKET="$socket" node "$sdk_dir/test/live-control.mjs"

echo "== bridge log =="
cat "$log"
echo "SDK e2e passed."
