#!/usr/bin/env bash
# Live provider test coverage for Antigravity models.
# For each model: (1) chat smoke -> expect token, (2) multi-turn tool smoke
# (two bash calls, exercises thought_signature replay) -> expect both outputs.
set -uo pipefail

JC=./target/selfdev/jcode
CHAT_PROMPT='Reply with exactly: SMOKE_OK'
TOOL_PROMPT="Run 'echo aa11' with bash, then in a SECOND separate bash call run 'echo bb22', then report both outputs."
CHAT_TIMEOUT=90
TOOL_TIMEOUT=160

# Models to test (chat-capable). Skip tab_*/chat_* (autocomplete/internal) and 'default' alias dup.
MODELS=(
  claude-opus-4-6-thinking
  claude-sonnet-4-6
  gemini-3.1-pro-high
  gemini-3.1-pro-low
  gemini-3-flash
  gemini-3-flash-agent
  gemini-3.5-flash-low
  gpt-oss-120b-medium
  gemini-2.5-flash
  gemini-2.5-flash-lite
  gemini-2.5-flash-thinking
  gemini-2.5-pro
  gemini-3.1-flash-lite
  gemini-3.5-flash-extra-low
  gemini-pro-agent
  default
)

printf "%-30s | %-8s | %-10s | %s\n" "MODEL" "CHAT" "TOOL" "NOTES"
printf -- "------------------------------------------------------------------------------------\n"

total=${#MODELS[@]}
i=0
for m in "${MODELS[@]}"; do
  i=$((i+1))
  echo "JCODE_PROGRESS {\"current\":$i,\"total\":$total,\"unit\":\"models\",\"message\":\"$m\"}" >&2

  # --- chat smoke ---
  chat_out=$(timeout "$CHAT_TIMEOUT" "$JC" run --provider antigravity -m "$m" --no-update --no-selfdev "$CHAT_PROMPT" 2>&1)
  chat_rc=$?
  if [[ $chat_rc -ne 0 ]]; then
    chat="FAIL"
  elif grep -q "SMOKE_OK" <<<"$chat_out"; then
    chat="PASS"
  else
    chat="NO_TOKEN"
  fi

  # --- tool smoke (multi-turn) ---
  note=""
  tool_out=$(timeout "$TOOL_TIMEOUT" "$JC" run --provider antigravity -m "$m" --no-update --no-selfdev "$TOOL_PROMPT" 2>&1)
  tool_rc=$?
  if [[ $tool_rc -ne 0 ]]; then
    tool="FAIL"
    note=$(grep -oiE "missing a thought_signature|400|schema|draft 2020|HTTP [0-9]+|error[^\"]{0,40}" <<<"$tool_out" | head -1 | tr -d '\n')
    [[ $tool_rc -eq 124 ]] && note="timeout"
  elif grep -q "aa11" <<<"$tool_out" && grep -q "bb22" <<<"$tool_out"; then
    tool="PASS"
  else
    tool="PARTIAL"
    note="missing one output"
  fi

  printf "%-30s | %-8s | %-10s | %s\n" "$m" "$chat" "$tool" "$note"
done
