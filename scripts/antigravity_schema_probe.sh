#!/usr/bin/env bash
# Isolate which JSON-Schema construct the Antigravity Cloud Code backend's
# Gemini->Anthropic translation rejects for Claude models.
#
# For each JCODE_SCHEMA_STRIP combination, run a single tool-using prompt
# against a Claude model and report PASS (tool ran) / SCHEMA (draft-2020-12
# rejection) / OTHER. Requires a valid Antigravity OAuth session.
set -uo pipefail

JC="${JC:-./target/selfdev/jcode}"
MODEL="${MODEL:-claude-sonnet-4-6}"
PROMPT="${PROMPT:-Run 'echo PROBE_OK' with the bash tool and report the output.}"
TIMEOUT="${TIMEOUT:-120}"

STRIPS=(
  ""                       # baseline (current behaviour)
  "anyof"
  "addprops"
  "numitems"
  "anyof,addprops"
  "anyof,addprops,numitems"
)

printf "%-28s | %-8s | %s\n" "STRIP" "RESULT" "DETAIL"
printf -- "----------------------------------------------------------------------\n"

for strip in "${STRIPS[@]}"; do
  out=$(JCODE_SCHEMA_STRIP="$strip" timeout "$TIMEOUT" \
    "$JC" run --provider antigravity -m "$MODEL" --no-update --no-selfdev "$PROMPT" 2>&1)
  rc=$?
  label="${strip:-<none>}"
  if [[ $rc -eq 124 ]]; then
    printf "%-28s | %-8s | %s\n" "$label" "TIMEOUT" ""
  elif grep -q "PROBE_OK" <<<"$out"; then
    printf "%-28s | %-8s | %s\n" "$label" "PASS" "tool executed"
  elif grep -qiE "draft 2020|2020-12|schema" <<<"$out"; then
    detail=$(grep -oiE "[^\"]{0,60}(draft 2020|2020-12|schema)[^\"]{0,60}" <<<"$out" | head -1 | tr -s ' \n' ' ')
    printf "%-28s | %-8s | %s\n" "$label" "SCHEMA" "$detail"
  else
    detail=$(grep -oiE "HTTP [0-9]+[^\"]{0,80}" <<<"$out" | head -1 | tr -s ' \n' ' ')
    printf "%-28s | %-8s | %s\n" "$label" "OTHER" "${detail:-see logs}"
  fi
done
