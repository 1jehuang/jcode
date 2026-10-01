#!/bin/bash
# judge-step4.sh — Tier-2 step 4 judge-judge agreement: backup model re-grades
# the full step-1 gold set; computes Cohen's kappa vs primary.
# Usage: judge-step4.sh <ledger> [backup-model] [primary-model]
set -u
HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
JUDGE_DIR="$HARNESS_DIR/judge"
source "$HARNESS_DIR/metering.sh"
LEDGER="$1"
BACKUP="${2:-mimo-v2.5-free}"
PRIMARY="${3:-mimo-v2.6-flash-free}"
ATOMS="$JUDGE_DIR/atoms.jsonl"
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

fail=0; n=0
while IFS= read -r line; do
    id="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    q="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    aj="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['atoms']))")"
    fj="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['forbidden']))")"
    ab="$(printf '%s' "$line" | python3 -c "import json,sys; print('yes' if json.load(sys.stdin)['abstention_required'] else 'no')")"
    ans="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['gold_answer'])")"
    sp="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['spans'])")"
    if out="$(grade_one "$BACKUP" "$id" "gold-judge2" "$q" "$aj" "$fj" "$ab" "$ans" "$sp")"; then
        printf '%s\n' "$out" >>"$LEDGER"
        n=$((n+1))
    else
        echo "TRANSPORT-FAIL $id: $GRADE_ERR" >&2; fail=1
    fi
done <"$ATOMS"
echo "step4 graded: $n (backup=$BACKUP)"
python3 - "$LEDGER" "$PRIMARY" "$BACKUP" <<'EOF'
import json,sys
led, primary, backup = sys.argv[1:4]
p={}; b={}
for line in open(led):
    line=line.strip()
    if not line.startswith('{'): continue
    r=json.loads(line)
    if r.get('record')!='judge': continue
    j=r['judge']
    if j['error'] or j['correct'] is None: continue
    if r['step']=='gold' and r['model']==primary: p[r['id']]=j['correct']
    elif r['step']=='gold-judge2' and r['model']==backup: b[r['id']]=j['correct']
ids=sorted(set(p)&set(b))
agree=sum(1 for i in ids if p[i]==b[i])
n=len(ids)
pa=agree/n if n else 0
# expected agreement from marginals
import collections
mp=sum(1 for i in ids if p[i])/n if n else 0
mb=sum(1 for i in ids if b[i])/n if n else 0
pe=mp*mb+(1-mp)*(1-mb)
kappa=(pa-pe)/(1-pe) if pe<1 else 1.0
print(json.dumps({"record":"judge-kappa","judges":[primary,backup],"n":n,
  "agree":agree,"po":round(pa,4),"pe":round(pe,4),"kappa":round(kappa,4)}))
EOF
exit $fail
