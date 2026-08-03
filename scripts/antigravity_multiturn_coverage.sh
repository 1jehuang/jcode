#!/usr/bin/env bash
# Live multi-turn coverage for Antigravity models.
#
# The single-shot smokes in antigravity_multiturn's sibling script
# (antigravity_live_coverage.sh) only prove one request works. The failure mode
# users actually hit is the *second and later* turn of a real session, where the
# provider must replay prior context, prior tool calls, and (for Gemini/Claude
# models behind Antigravity) opaque `thought_signature` blobs attached to
# earlier assistant turns. A model can pass a one-shot smoke and still 400 on
# turn 2.
#
# So each model here runs one resumed session of 4 turns that exercises,
# in order:
#   turn 1  plain chat, establishes a fact the model must carry forward
#   turn 2  recall of turn 1 (context replay across resume)
#   turn 3  tool call, then recall of the turn-1 fact in the same turn
#           (tool-result + thought_signature replay on top of history)
#   turn 4  recall of both the turn-1 fact and the turn-3 tool output
#           (full history replay after a tool turn: the usual 400 site)
#
# Exit status is non-zero when any model fails, so this is usable as a gate.
set -uo pipefail

JC=${JC:-./target/selfdev/jcode}
TURN_TIMEOUT=${TURN_TIMEOUT:-120}
MODELS_OVERRIDE=${MODELS:-}

# The magic word is checked verbatim, so a model that drops history fails loudly
# instead of producing a plausible-looking answer.
MAGIC="ZEBRA42"
TOOL_TOKEN="TOOLOUT77"

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
  gemini-3.1-flash-lite
  gemini-3.5-flash-extra-low
  gemini-pro-agent
  default
)
if [[ -n "$MODELS_OVERRIDE" ]]; then
  read -r -a MODELS <<<"$MODELS_OVERRIDE"
fi

# Run one turn. Echoes the assistant text; sets TURN_SESSION on turn 1.
# usage: run_turn <model> <session-or-empty> <prompt>
run_turn() {
  local model=$1 session=$2 prompt=$3 out
  if [[ -z "$session" ]]; then
    out=$(timeout "$TURN_TIMEOUT" "$JC" run --provider antigravity -m "$model" \
      --no-update --no-selfdev --json "$prompt" 2>&1)
  else
    out=$(timeout "$TURN_TIMEOUT" "$JC" run --provider antigravity -m "$model" \
      --no-update --no-selfdev --json --resume "$session" "$prompt" 2>&1)
  fi
  local rc=$?
  TURN_RAW=$out
  if [[ $rc -ne 0 ]]; then
    TURN_RC=$rc
    TURN_TEXT=""
    return 1
  fi
  TURN_RC=0
  TURN_SESSION=$(python3 -c '
import json,sys
try:
    print(json.loads(sys.stdin.read()).get("session_id",""))
except Exception:
    print("")' <<<"$out")
  TURN_TEXT=$(python3 -c '
import json,sys
try:
    print(json.loads(sys.stdin.read()).get("text",""))
except Exception:
    print("")' <<<"$out")
  [[ -n "$TURN_TEXT" ]]
}

# Short, greppable reason for a failed turn.
failure_note() {
  grep -oiE "thought_signature[^\"]{0,40}|HTTP [0-9]{3}[^\"]{0,40}|400[^\"]{0,30}|schema[^\"]{0,30}|error[^\"]{0,40}" \
    <<<"$TURN_RAW" | head -1 | tr -d '\n' | cut -c1-70
}

printf "%-30s | %-6s | %-6s | %-6s | %-6s | %s\n" \
  "MODEL" "T1" "T2" "T3" "T4" "NOTES"
printf -- "--------------------------------------------------------------------------------------------\n"

failures=0
total=${#MODELS[@]}
i=0
for m in "${MODELS[@]}"; do
  i=$((i + 1))
  echo "JCODE_PROGRESS {\"current\":$i,\"total\":$total,\"unit\":\"models\",\"message\":\"$m\"}" >&2

  t1=- t2=- t3=- t4=- note="" session=""

  # Turn 1: establish a fact the later turns must recall.
  if run_turn "$m" "" "Remember this magic word for later: $MAGIC. Reply with exactly: READY"; then
    if grep -q "READY" <<<"$TURN_TEXT"; then t1=PASS; else t1=NOTOK; note="turn1 no READY"; fi
    session=$TURN_SESSION
  else
    t1=FAIL
    note=$(failure_note)
    [[ $TURN_RC -eq 124 ]] && note="turn1 timeout"
  fi

  # Turn 2: plain context replay across a resume.
  if [[ $t1 == PASS && -n $session ]]; then
    if run_turn "$m" "$session" "What was the magic word? Reply with only the word."; then
      if grep -q "$MAGIC" <<<"$TURN_TEXT"; then t2=PASS; else t2=NOTOK; note="turn2 lost context"; fi
    else
      t2=FAIL
      note=$(failure_note)
      [[ $TURN_RC -eq 124 ]] && note="turn2 timeout"
    fi
  fi

  # Turn 3: tool call layered on top of existing history, plus recall. This is
  # where thought_signature replay first has to survive a tool round-trip.
  if [[ $t2 == PASS ]]; then
    if run_turn "$m" "$session" \
      "Use the bash tool to run 'echo $TOOL_TOKEN', then reply with the command output followed by the magic word."; then
      if grep -q "$TOOL_TOKEN" <<<"$TURN_TEXT" && grep -q "$MAGIC" <<<"$TURN_TEXT"; then
        t3=PASS
      else
        t3=NOTOK
        note="turn3 missing tool output or magic word"
      fi
    else
      t3=FAIL
      note=$(failure_note)
      [[ $TURN_RC -eq 124 ]] && note="turn3 timeout"
    fi
  fi

  # Turn 4: replay the whole history *after* a tool turn. This is the turn that
  # 400s when signed assistant parts are dropped or reordered.
  if [[ $t3 == PASS ]]; then
    if run_turn "$m" "$session" \
      "Without using any tools, reply with the magic word and the command output from earlier, separated by a space."; then
      if grep -q "$TOOL_TOKEN" <<<"$TURN_TEXT" && grep -q "$MAGIC" <<<"$TURN_TEXT"; then
        t4=PASS
      else
        t4=NOTOK
        note="turn4 lost history after tool turn"
      fi
    else
      t4=FAIL
      note=$(failure_note)
      [[ $TURN_RC -eq 124 ]] && note="turn4 timeout"
    fi
  fi

  [[ $t1 == PASS && $t2 == PASS && $t3 == PASS && $t4 == PASS ]] || failures=$((failures + 1))
  printf "%-30s | %-6s | %-6s | %-6s | %-6s | %s\n" "$m" "$t1" "$t2" "$t3" "$t4" "$note"
done

printf -- "--------------------------------------------------------------------------------------------\n"
if [[ $failures -eq 0 ]]; then
  echo "All ${#MODELS[@]} model(s) passed 4-turn multi-turn coverage."
else
  echo "$failures of ${#MODELS[@]} model(s) failed multi-turn coverage."
fi
exit $((failures > 0))
