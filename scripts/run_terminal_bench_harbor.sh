#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
DEFAULT_BINARY_DIR=${JCODE_HARBOR_BINARY_DIR:-/tmp/jcode-compat-dist}
DEFAULT_BINARY_PATH=${JCODE_HARBOR_BINARY:-$DEFAULT_BINARY_DIR/jcode-linux-x86_64}
DEFAULT_MODEL=${JCODE_TB_MODEL:-openai/gpt-5.4}
DEFAULT_PATH=${JCODE_TB_PATH:-/tmp/terminal-bench-2}

have_model=0
have_agent_import=0
have_task_source=0

for arg in "$@"; do
  case "$arg" in
    --model|-m)
      have_model=1
      ;;
    --agent-import-path)
      have_agent_import=1
      ;;
    --path|-p|--dataset|-d|--task|-t)
      have_task_source=1
      ;;
  esac
done

if [[ ! -x "$DEFAULT_BINARY_PATH" ]]; then
  echo "Building Linux-compatible jcode binary into $DEFAULT_BINARY_DIR" >&2
  "$REPO_ROOT/scripts/build_linux_compat.sh" "$DEFAULT_BINARY_DIR"
fi

OPENAI_AUTH=${JCODE_HARBOR_OPENAI_AUTH:-$HOME/.jcode/openai-auth.json}
if [[ ! -f "$OPENAI_AUTH" ]]; then
  echo "OpenAI OAuth file not found at $OPENAI_AUTH" >&2
  exit 1
fi

export PYTHONPATH="$REPO_ROOT/scripts${PYTHONPATH:+:$PYTHONPATH}"
export JCODE_HARBOR_BINARY="$DEFAULT_BINARY_PATH"
export JCODE_HARBOR_OPENAI_AUTH="$OPENAI_AUTH"
export JCODE_OPENAI_REASONING_EFFORT=${JCODE_OPENAI_REASONING_EFFORT:-high}
export JCODE_OPENAI_SERVICE_TIER=${JCODE_OPENAI_SERVICE_TIER:-priority}
export JCODE_NO_TELEMETRY=${JCODE_NO_TELEMETRY:-1}

cmd=(uvx harbor run)
if [[ $have_task_source -eq 0 ]]; then
  cmd+=(--path "$DEFAULT_PATH")
fi
if [[ $have_agent_import -eq 0 ]]; then
  cmd+=(--agent-import-path jcode_harbor_agent:JcodeHarborAgent)
fi
if [[ $have_model -eq 0 ]]; then
  cmd+=(--model "$DEFAULT_MODEL")
fi
cmd+=("$@")

{
  echo "Running Harbor with jcode adapter"
  echo "  binary: $JCODE_HARBOR_BINARY"
  echo "  auth:   $JCODE_HARBOR_OPENAI_AUTH"
  echo "  model:  ${DEFAULT_MODEL}"
} >&2

exec "${cmd[@]}"
