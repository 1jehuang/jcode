#!/bin/bash
# judge-retry.sh — re-grade specific (id,step) pairs with a chosen model, append to ledger.
# Usage: judge-retry.sh <ledger> <model> <id:step> [<id:step> ...]
set -u
HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
JUDGE_DIR="$HARNESS_DIR/judge"
source "$HARNESS_DIR/metering.sh"
LEDGER="$1"; MODEL="$2"; shift 2
ATOMS="$JUDGE_DIR/atoms.jsonl"
PARAS="$JUDGE_DIR/paraphrases.jsonl"
if [ -z "${OPENAI_COMPAT_API_KEY:-}" ] && [ -f "$HOME/.config/jcode/opencode-proxy.env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.config/jcode/opencode-proxy.env"
fi
PROXY_BASE="${PROXY_BASE:-http://127.0.0.1:8787/v1}"
JUDGE_SYSTEM="You are a strict factuality grader for a memory-system acceptance harness. Verify short answers against pre-registered atomic facts. You are NOT generous: topical-but-vague answers FAIL. Output JSON only."
TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT
# shellcheck disable=SC1091
source <(sed -n '/^build_user_prompt/,/^}/p;/^grade_one/,/^}/p' "$HARNESS_DIR/judge-calibrate.sh")

fail=0
for pair in "$@"; do
    id="${pair%%:*}"; step="${pair##*:}"
    if [[ "$step" == paraphrase* ]] || [[ "$id" == PAR-* ]]; then
        src="$PARAS"; key="$id"; ans_field="answer"
        base_id="$(python3 -c "import json,sys; print([json.loads(l) for l in open('$PARAS') if json.loads(l)['id']=='$id'][0]['item_id'])")"
        base="$(grep -F "\"id\": \"$base_id\"" "$ATOMS" | head -1)"
    else
        src="$ATOMS"; key="$id"
        base="$(grep -F "\"id\": \"$id\"" "$ATOMS" | head -1)"
    fi
    q="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    aj="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['atoms']))")"
    fj="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['forbidden']))")"
    ab="$(printf '%s' "$base" | python3 -c "import json,sys; print('yes' if json.load(sys.stdin)['abstention_required'] else 'no')")"
    sp="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['spans'])")"
    case "$step" in
        gold) ans="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['gold_answer'])")";;
        paraphrase) ans="$(python3 -c "import json; print([json.loads(l) for l in open('$PARAS') if json.loads(l)['id']=='$id'][0]['answer'])")";;
        adv-vague) ans="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['vague_answer'])")";;
        adv-forbidden) ans="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['forbidden_answer'])")";;
        adv-markerless) ans="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['markerless_answer'])")";;
        *) echo "UNKNOWN-STEP $pair" >&2; fail=1; continue;;
    esac
    if out="$(grade_one "$MODEL" "$id" "$step" "$q" "$aj" "$fj" "$ab" "$ans" "$sp")"; then
        printf '%s\n' "$out" >>"$LEDGER"
        v="$(printf '%s' "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); print(str(d['judge']['correct'])+'|'+str(d['judge']['error']))")"
        echo "RETRY $pair [$MODEL] -> $v"
    else
        echo "TRANSPORT-FAIL $pair: $GRADE_ERR" >&2; fail=1
    fi
done
exit $fail
