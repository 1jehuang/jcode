#!/bin/bash
# judge-step3.sh — Tier-2 step 3 adversarial sweep only (appends to an existing ledger).
# Reuses grade_one/build_user_prompt from judge-calibrate.sh via sourcing.
# Usage: judge-step3.sh <ledger> [model]
set -u
HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
JUDGE_DIR="$HARNESS_DIR/judge"
source "$HARNESS_DIR/metering.sh"
LEDGER="$1"
MODEL="${2:-mimo-v2.6-flash-free}"
ATOMS="$JUDGE_DIR/atoms.jsonl"
if [ -z "${OPENAI_COMPAT_API_KEY:-}" ] && [ -f "$HOME/.config/jcode/opencode-proxy.env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.config/jcode/opencode-proxy.env"
fi
PROXY_BASE="${PROXY_BASE:-http://127.0.0.1:8787/v1}"
# pull shared functions (guard: only function defs + JUDGE_SYSTEM + TMPD setup)
JUDGE_SYSTEM="You are a strict factuality grader for a memory-system acceptance harness. Verify short answers against pre-registered atomic facts. You are NOT generous: topical-but-vague answers FAIL. Output JSON only."
TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT
# shellcheck disable=SC1091
source <(sed -n '/^build_user_prompt/,/^}/p;/^grade_one/,/^}/p' "$HARNESS_DIR/judge-calibrate.sh")

s3_vrej=0; s3_vtot=0; s3_frej=0; s3_ftot=0; s3_mrej=0; s3_mtot=0
fail=0
while IFS= read -r line; do
    id="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    q="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    aj="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['atoms']))")"
    fj="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['forbidden']))")"
    ab="$(printf '%s' "$line" | python3 -c "import json,sys; print('yes' if json.load(sys.stdin)['abstention_required'] else 'no')")"
    sp="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['spans'])")"
    for kind in vague_answer forbidden_answer markerless_answer; do
        ans="$(printf '%s' "$line" | KIND="$kind" python3 -c "
import json,sys,os
d=json.load(sys.stdin); v=d.get(os.environ['KIND']); print(v if v else '')
")"
        [ -z "$ans" ] && continue
        short="${kind%_answer}"
        if out="$(grade_one "$MODEL" "$id" "adv-$short" "$q" "$aj" "$fj" "$ab" "$ans" "$sp")"; then
            printf '%s\n' "$out" >>"$LEDGER"
            v="$(printf '%s' "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); print(str(d['judge']['correct']))")"
            case "$short" in
                vague) s3_vtot=$((s3_vtot+1)); if [ "$v" = "False" ]; then s3_vrej=$((s3_vrej+1)); else echo "VAGUE-PASS $id"; fi;;
                forbidden) s3_ftot=$((s3_ftot+1)); if [ "$v" = "False" ]; then s3_frej=$((s3_frej+1)); else echo "FORB-PASS $id"; fi;;
                markerless) s3_mtot=$((s3_mtot+1)); if [ "$v" = "False" ]; then s3_mrej=$((s3_mrej+1)); else echo "MARK-PASS $id"; fi;;
            esac
        else
            echo "TRANSPORT-FAIL $id/$short: $GRADE_ERR" >&2; fail=1
        fi
    done
done <"$ATOMS"
echo "step3 vague-reject: $s3_vrej/$s3_vtot | forbidden-reject: $s3_frej/$s3_ftot | markerless-reject: $s3_mrej/$s3_mtot"
exit $fail
