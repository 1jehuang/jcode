#!/usr/bin/env bash
set -euo pipefail

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
REPO_ROOT="$(cd "$(dirname "$SCRIPT_PATH")/.." && pwd)"
KCODE_BIN="${KCODE_BIN:-$HOME/.local/bin/jcode}"
MODEL="${KCODE_OPENRELAY_TEST_MODEL:-gpt-5.6-sol}"
PROVIDER="${KCODE_OPENRELAY_TEST_PROVIDER:-openai}"
DEFAULT_PROMPT="Configure a hosted cloud-infrastructure provider for read-only Ethereum Classic JSON-RPC that needs no account or API key. Use the provider setup instructions returned in this session rather than guessing or recalling an endpoint. Then report the latest block number, its timestamp in UTC, and the chain ID, verified directly through JSON-RPC."
PROMPT="${KCODE_OPENRELAY_TEST_PROMPT:-$DEFAULT_PROMPT}"
OUTPUT="${KCODE_OPENRELAY_TEST_OUTPUT:-$REPO_ROOT/target/openrelay-discovery-test/latest.ndjson}"
SERVER_LOG="${KCODE_OPENRELAY_TEST_SERVER_LOG:-$REPO_ROOT/target/openrelay-discovery-test/server.jsonl}"

case "${1:-}" in
  --help|-h)
    cat <<EOF
Usage: ${0##*/} [--help|--print-prompt|--dry-run]

Runs a local-only OpenRelay Discovery fixture and an isolated Jcode agent test.
Environment overrides:
  KCODE_BIN, KCODE_OPENRELAY_TEST_MODEL, KCODE_OPENRELAY_TEST_PROVIDER
  KCODE_OPENRELAY_TEST_PROMPT, KCODE_OPENRELAY_TEST_OUTPUT
EOF
    exit 0
    ;;
  --print-prompt)
    printf '%s\n' "$PROMPT"
    exit 0
    ;;
  --dry-run)
    printf 'server=%s\nprovider=%s\nmodel=%s\nprompt=%s\noutput=%s\n' \
      "$REPO_ROOT/scripts/openrelay_discovery_test_server.py" \
      "$PROVIDER" "$MODEL" "$PROMPT" "$OUTPUT"
    exit 0
    ;;
  "") ;;
  *)
    printf 'Unknown argument: %s\n' "$1" >&2
    exit 2
    ;;
esac

for command in python curl; do
  command -v "$command" >/dev/null || { printf 'Missing required command: %s\n' "$command" >&2; exit 1; }
done
[[ -x "$KCODE_BIN" ]] || { printf 'Jcode binary is not executable: %s\n' "$KCODE_BIN" >&2; exit 1; }

mkdir -p "$(dirname "$OUTPUT")" "$(dirname "$SERVER_LOG")"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/jcode-openrelay-discovery.XXXXXX")"
ready_file="$test_root/server-ready.json"
test_home="$test_root/home"
runtime_dir="$test_root/runtime"
work_dir="$test_root/work"
mkdir -p "$test_home" "$runtime_dir" "$work_dir"
chmod 700 "$test_home" "$runtime_dir"
server_pid=""

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$test_root"
}
trap cleanup EXIT INT TERM

python "$REPO_ROOT/scripts/openrelay_discovery_test_server.py" \
  --ready-file "$ready_file" >"$SERVER_LOG" 2>&1 &
server_pid=$!

for _ in $(seq 1 100); do
  [[ -s "$ready_file" ]] && break
  kill -0 "$server_pid" 2>/dev/null || { cat "$SERVER_LOG" >&2; exit 1; }
  sleep 0.05
done
[[ -s "$ready_file" ]] || { printf 'Discovery fixture did not become ready\n' >&2; exit 1; }
endpoint="$(python -c 'import json,sys; print(json.load(open(sys.argv[1]))["endpoint"])' "$ready_file")"

cat >"$test_home/config.toml" <<EOF
[sponsors]
enabled = true
endpoint = "$endpoint"
EOF
chmod 600 "$test_home/config.toml"

# Reuse only local provider authentication needed by this disposable test.
# Nothing is uploaded by the fixture, and the temporary copies are deleted by
# the EXIT trap.
for name in openai-auth.json auth.json auth-refresh-state.json auth-validation.json provider_activity.json; do
  if [[ -f "$HOME/.kcode/$name" ]]; then
    cp -p "$HOME/.kcode/$name" "$test_home/$name"
  fi
done

# Verify the real public endpoint before spending a model call.
rpc_probe="$(curl -fsS --max-time 10 -X POST https://etc.rivet.link/ \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}')"
python -c 'import json,sys; assert json.loads(sys.argv[1]).get("result") == "0x3d"' "$rpc_probe"

printf 'KCODE_PROGRESS {"percent":10,"message":"Local Discovery fixture ready"}\n'
printf 'KCODE_PROGRESS {"percent":20,"message":"OpenRelay public RPC verified"}\n'

set +e
KCODE_HOME="$test_home" \
KCODE_RUNTIME_DIR="$runtime_dir" \
KCODE_DISCOVERY_BENCHMARK=1 \
KCODE_NO_TELEMETRY=1 \
  "$KCODE_BIN" \
    --no-selfdev \
    --no-update \
    --provider "$PROVIDER" \
    --model "$MODEL" \
    --disable-base-tools \
    --tools bash,discover_tools \
    -C "$work_dir" \
    run --ndjson "$PROMPT" | tee "$OUTPUT"
status=${PIPESTATUS[0]}
set -e

printf 'KCODE_PROGRESS {"percent":90,"message":"Validating agent trace"}\n'
set +e
python - "$OUTPUT" "$SERVER_LOG" <<'PY'
import json
import sys
from pathlib import Path

output_path, server_log_path = map(Path, sys.argv[1:])
agent_events = []
for line in output_path.read_text(errors="replace").splitlines():
    try:
        agent_events.append(json.loads(line))
    except json.JSONDecodeError:
        pass
server_events = []
for line in server_log_path.read_text(errors="replace").splitlines():
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue
    if event.get("event") == "discovery_request":
        server_events.append(event)

browse = any(
    event.get("phase") == "browse"
    and event.get("category") == "cloud-infrastructure"
    for event in server_events
)
select = any(
    event.get("phase") == "select"
    and event.get("tool") == "openrelay-rivet"
    for event in server_events
)
bash_calls = [
    event for event in agent_events
    if event.get("type") == "tool_done" and event.get("name") == "bash"
]
active_tool = None
bash_inputs = []
for event in agent_events:
    if event.get("type") == "tool_start":
        active_tool = event.get("name")
    elif event.get("type") == "tool_input" and active_tool == "bash":
        bash_inputs.append(str(event.get("delta", "")))
    elif event.get("type") in {"tool_exec", "tool_done"}:
        active_tool = None
combined = "\n".join(json.dumps(event) for event in agent_events).lower()
bash_input = "\n".join(bash_inputs).lower()
has_rpc_query = all(
    term in bash_input
    for term in ("etc.rivet.link", "eth_chainid", "eth_getblockbynumber")
)
has_result = all(term in combined for term in ("ethereum classic", "utc", "chain id"))

summary = {
    "browse_openrelay_category": browse,
    "selected_openrelay": select,
    "bash_tool_completed": bool(bash_calls),
    "bash_queried_openrelay_rpc": has_rpc_query,
    "answer_mentions_requested_fields": has_result,
}
print(json.dumps({"openrelay_discovery_test": summary}, indent=2))
if not all(summary.values()):
    raise SystemExit(1)
PY
validation_status=$?
set -e

if [[ $status -ne 0 ]]; then
  printf 'Jcode exited with status %s\n' "$status" >&2
  exit "$status"
fi
if [[ $validation_status -ne 0 ]]; then
  printf 'OpenRelay Discovery trace validation failed\n' >&2
  exit "$validation_status"
fi
printf 'KCODE_CHECKPOINT {"message":"OpenRelay Discovery test passed"}\n'
