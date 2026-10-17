#!/usr/bin/env bash
set -euo pipefail

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
REPO_ROOT="$(cd "$(dirname "$SCRIPT_PATH")/.." && pwd)"
KCODE_BIN="${KCODE_BIN:-$HOME/.local/bin/jcode}"
DEMO_DIR="${KCODE_AGENTCARD_DEMO_DIR:-/tmp/jcode-agentcard-discovery-demo}"
MODEL="${KCODE_AGENTCARD_DEMO_MODEL:-gpt-5.6-sol}"
LOG_FILE="${KCODE_AGENTCARD_DEMO_LOG:-/tmp/jcode-agentcard-discovery-demo.log}"
PROMPT='Use `./bin/jcode-demo-shop` to see whether this shop has a USB-C laptop charger for $50 or less and get it for me. Work through any prerequisites, but ask me for confirmation immediately before actually creating or funding a prepaid card, making a payment, or placing the order.'

case "${1:-}" in
  --help|-h)
    printf 'Usage: %s [--help|--print-prompt|--dry-run]\n' "${0##*/}"
    exit 0
    ;;
  --print-prompt)
    printf '%s\n' "$PROMPT"
    exit 0
    ;;
  --dry-run)
    printf 'repo=%s\nshop=%s\nprompt=%s\n' "$REPO_ROOT" "$REPO_ROOT/scripts/demo_shop.py" "$PROMPT"
    test -x "$REPO_ROOT/scripts/demo_shop.py"
    exit 0
    ;;
  "") ;;
  *)
    printf 'Unknown argument: %s\n' "$1" >&2
    exit 2
    ;;
esac

mkdir -p "$DEMO_DIR/bin"
ln -sfn "$REPO_ROOT/scripts/demo_shop.py" "$DEMO_DIR/bin/jcode-demo-shop"
chmod +x "$REPO_ROOT/scripts/demo_shop.py"
export PATH="$DEMO_DIR/bin:$PATH"
export KCODE_DEMO_SHOP_STATE="$DEMO_DIR/shop-state.json"
jcode-demo-shop reset >>"$LOG_FILE" 2>&1

before_file="$(mktemp)"
after_file="$(mktemp)"
trap 'rm -f "$before_file" "$after_file"' EXIT

# Snapshot connected clients so prompt delivery targets only the new demo.
"$KCODE_BIN" debug clients:map >"$before_file" 2>>"$LOG_FILE" || printf '{"clients":[]}' >"$before_file"

# The mock shop and Discovery are the only capabilities exposed. The shop has
# no networking, account, payment, or order-placement implementation.
kitty \
  --class jcode-agentcard-demo \
  --title "Jcode AgentCard Discovery Demo" \
  --directory "$DEMO_DIR" \
  "$KCODE_BIN" \
  --no-selfdev \
  --no-update \
  --model "$MODEL" \
  --disable-base-tools \
  --tools bash,discover_tools \
  --cwd "$DEMO_DIR" \
  >>"$LOG_FILE" 2>&1 &

session_id=""
for _ in $(seq 1 100); do
  if "$KCODE_BIN" debug clients:map >"$after_file" 2>>"$LOG_FILE"; then
    session_id="$(python - "$before_file" "$after_file" "$DEMO_DIR" <<'PY'
import json
import sys

before_path, after_path, demo_dir = sys.argv[1:]


def load(path):
    try:
        with open(path, encoding="utf-8") as file:
            return json.load(file)
    except Exception:
        return {"clients": []}


before_ids = {client.get("session_id") for client in load(before_path).get("clients", [])}
candidates = [
    client
    for client in load(after_path).get("clients", [])
    if client.get("session_id") not in before_ids
    and client.get("working_dir") == demo_dir
]
candidates.sort(key=lambda client: client.get("connected_secs_ago", 999999))
if candidates:
    print(candidates[0]["session_id"])
PY
)"
  fi
  [[ -n "$session_id" ]] && break
  sleep 0.2
done

if [[ -z "$session_id" ]]; then
  notify-send "AgentCard demo could not start" "Jcode did not register a fresh demo session. See $LOG_FILE" 2>/dev/null || true
  exit 1
fi

# Targeted debug delivery avoids timing-sensitive keyboard simulation.
for _ in $(seq 1 25); do
  if "$KCODE_BIN" debug --session "$session_id" "client:message:$PROMPT" >>"$LOG_FILE" 2>&1; then
    exit 0
  fi
  sleep 0.2
done

notify-send "AgentCard demo prompt failed" "The Jcode window opened, but prompt delivery failed. See $LOG_FILE" 2>/dev/null || true
exit 1
