#!/bin/bash
# judge-calibrate.sh — Tier-2 atomic judge calibration driver (02 steps 1-5).
# REPORT-ONLY: touches nothing in run.sh, pin-five.json, or Tier-1 gates.
# Transport: direct HTTPS to the proxy /v1/chat/completions with the pinned
# model (transport-equivalent to Sidecar::complete_via_provider; see PLAN.md).
# No OpenRouter at any hop. Blind sets never opened (atoms come from
# harness/judge/gen_atoms.py over DEV fixtures only).
#
# Usage: harness/judge-calibrate.sh [primary-model] [backup-model]
# Defaults: mimo-v2.6-flash-free / mimo-v2.5-free.
# Env: OPENAI_COMPAT_API_KEY (or sourced from ~/.config/jcode/opencode-proxy.env),
#      PROXY_BASE (default http://127.0.0.1:8787/v1), PRICE_PER_1K (default 0.002).
# Exit: 0 = calibration completed and ledger written (verdict in summary);
#       2 = transport failure (UNKNOWN, exact error in log, <=2 retries/item).

set -u
HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
JUDGE_DIR="$HARNESS_DIR/judge"
# shellcheck disable=SC1091
source "$HARNESS_DIR/metering.sh"

PRIMARY_MODEL="${1:-mimo-v2.6-flash-free}"
BACKUP_MODEL="${2:-mimo-v2.5-free}"
PROXY_BASE="${PROXY_BASE:-http://127.0.0.1:8787/v1}"
if [ -z "${OPENAI_COMPAT_API_KEY:-}" ] && [ -f "$HOME/.config/jcode/opencode-proxy.env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.config/jcode/opencode-proxy.env"
fi
if [ -z "${OPENAI_COMPAT_API_KEY:-}" ]; then
    echo "TRANSPORT-ERROR: no OPENAI_COMPAT_API_KEY (proxy server key missing)" >&2
    exit 2
fi

TS="$(date -u +%Y%m%dT%H%M%SZ)"
LEDGER="$HARNESS_DIR/ledger/judge-$TS.jsonl"
ATOMS="$JUDGE_DIR/atoms.jsonl"
PARAS="$JUDGE_DIR/paraphrases.jsonl"
TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

# 02 prompt skeleton verbatim (02 REPORT Tier-2 §Prompt).
JUDGE_SYSTEM="You are a strict factuality grader for a memory-system acceptance harness. Verify short answers against pre-registered atomic facts. You are NOT generous: topical-but-vague answers FAIL. Output JSON only."

build_user_prompt() {
    # args: question, atoms_json, forbidden_json, abstention(yes/no), answer, spans
    python3 - "$1" "$2" "$3" "$4" "$5" "$6" <<'EOF'
import json,sys
question, atoms_j, forb_j, abst, answer, spans = sys.argv[1:7]
atoms = json.loads(atoms_j); forb = json.loads(forb_j)
alines = "\n".join(f"{i+1}. {a}" for i,a in enumerate(atoms)) or "(none — abstention item)"
flines = "\n".join(f"{i+1}. {s}" for i,s in enumerate(forb)) or "none"
print(f"""Question: {question}
Gold atoms (EACH must be supported):
{alines}
Forbidden strings (EACH must be ABSENT; any hit = FAIL):
{flines}
Abstention required: {abst}""")
if abst == "yes":
    print("marker must come from: no record/forgotten/unknown/No memories found")
print(f"""Candidate answer: {answer}
Retrieved spans (context only, not graded): {spans}
Steps: 1. Per atom, quote the minimal supporting substring or NO-EVIDENCE (topical mention without the specific value = NO-EVIDENCE; numbers/names/codes must match verbatim). 2. Per forbidden string: PRESENT/ABSENT (case-sensitive). 3. Verdict: PASS iff all atoms quoted AND all forbidden absent (AND marker present if abstention required). Any forbidden PRESENT = FAIL (invalid-reuse veto, regardless of atoms).
JSON only: {{"atoms":[{{"atom":..,"quote":..,"supported":bool}}],"forbidden":[{{"string":..,"present":bool}}],"abstention_marker":bool|null,"analysis":"<one sentence>","correct":bool,"invalid_reuse":bool}}""")
EOF
}

# grade_one <model> <id> <step> <question> <atoms_json> <forbidden_json> <abst> <answer> <spans>
# stdout: ledger JSON line. Global GRADE_ERR set on transport failure.
GRADE_ERR=""
grade_one() {
    local model="$1" id="$2" step="$3" question="$4" atoms_j="$5" forb_j="$6" abst="$7" answer="$8" spans="$9"
    local user_prompt resp content verdict correct invalid_reuse verr
    user_prompt="$(build_user_prompt "$question" "$atoms_j" "$forb_j" "$abst" "$answer" "$spans")"
    local req="$TMPD/req.json"
    python3 - "$model" "$JUDGE_SYSTEM" "$user_prompt" >"$req" <<'EOF'
import json,sys
model, system, user = sys.argv[1:4]
print(json.dumps({"model": model,
  "messages": [{"role":"system","content":system},{"role":"user","content":user}],
  "temperature": 0, "max_tokens": 512}))
EOF
    local attempt=0 http=""
    while [ $attempt -lt 2 ]; do
        attempt=$((attempt+1))
        if http="$(curl -s -m 120 -w '\n%{http_code}' "$PROXY_BASE/chat/completions" \
            -H "Authorization: Bearer $OPENAI_COMPAT_API_KEY" \
            -H "Content-Type: application/json" -d @"$req" 2>"$TMPD/curl.err")"; then
            break
        fi
        sleep 3
    done
    if [ -z "$http" ]; then
        GRADE_ERR="curl-failed: $(cat "$TMPD/curl.err" 2>/dev/null | head -c 200)"
        return 1
    fi
    local code
    code="$(printf '%s' "$http" | tail -1)"
    resp="$(printf '%s' "$http" | sed '$d')"
    if [ "$code" != "200" ]; then
        GRADE_ERR="http-$code: $(printf '%s' "$resp" | head -c 300)"
        return 1
    fi
    content="$(printf '%s' "$resp" | python3 -c "
import json,sys
d = json.load(sys.stdin)
try:
    m = d['choices'][0]['message']
    c = m.get('content') or ''
    if not c.strip(): c = m.get('reasoning_content') or ''
    print(c)
except Exception as e:
    print('EXTRACT-ERROR: '+str(e), file=sys.stderr); sys.exit(1)
")"
    # strict verdict parse + ledger line in ONE python step (never silent pass)
    ledger_line="$(printf '%s' "$content" | GRADE_ID="$id" GRADE_STEP="$step" GRADE_MODEL="$model" GRADE_ATOMS="$atoms_j" GRADE_PROMPT="$JUDGE_SYSTEM $user_prompt" python3 -c "
import json,sys,os,math
raw = sys.stdin.read()
gid = os.environ['GRADE_ID']; gstep = os.environ['GRADE_STEP']
gmodel = os.environ['GRADE_MODEL']; atoms_j = os.environ['GRADE_ATOMS']
prompt = os.environ['GRADE_PROMPT']
start = raw.find('{'); end = raw.rfind('}')
verr = None; correct = None; iru = None; af = 0
try:
    assert start>=0 and end>start, 'no-json-object'
    v = json.loads(raw[start:end+1])
    assert isinstance(v.get('correct'), bool), 'correct-not-bool'
    correct = v['correct']; iru = v.get('invalid_reuse')
    af = sum(1 for a in v.get('atoms',[]) if a.get('supported') is True)
except Exception as e:
    verr = 'parse:'+str(e)[:80]
at = len(json.loads(atoms_j))
pt = (len(prompt)+3)//4; ct = (len(raw)+3)//4
dol = round((pt+ct)*float(os.environ.get('PRICE_PER_1K','0.002'))/1000, 6)
print(json.dumps({'record':'judge','id':gid,'step':gstep,'model':gmodel,
 'judge':{'correct':correct,'invalid_reuse':iru,'error':verr,
   'model':gmodel,'atoms_found':af,'atoms_total':at},
 'tokens':pt+ct,'dollars':dol}))
")"
    printf '%s\n' "$ledger_line"
    return 0
}

# --- run header ---
python3 - "$TS" "$PRIMARY_MODEL" "$BACKUP_MODEL" <<'EOF' >"$LEDGER"
import json,sys,subprocess
ts, primary, backup = sys.argv[1:4]
try:
    sha = subprocess.check_output(["git","rev-parse","--short","HEAD"],text=True).strip()
except Exception: sha = "unknown"
print(json.dumps({"record":"judge-run","started_at":ts,"repo_sha":sha,
  "primary_model":primary,"backup_model":backup,"tier":"2-calibration-report-only",
  "transport":"proxy-direct-transport-equivalent","openrouter":"excluded"}))
EOF

fail=0
step1_pass=0; step1_total=0; step1_err=0
declare -A STEP1_VERDICTS
echo "=== STEP 1: gold sweep ($PRIMARY_MODEL) ==="
while IFS= read -r line; do
    id="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    q="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    aj="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['atoms']))")"
    fj="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['forbidden']))")"
    ab="$(printf '%s' "$line" | python3 -c "import json,sys; print('yes' if json.load(sys.stdin)['abstention_required'] else 'no')")"
    ans="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['gold_answer'])")"
    sp="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['spans'])")"
    if out="$(grade_one "$PRIMARY_MODEL" "$id" "gold" "$q" "$aj" "$fj" "$ab" "$ans" "$sp")"; then
        printf '%s\n' "$out" >>"$LEDGER"
        v="$(printf '%s' "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); print(str(d['judge']['correct'])+'|'+str(d['judge']['error']))")"
        STEP1_VERDICTS["$id"]="$v"
        step1_total=$((step1_total+1))
        if [ "$v" = "True|None" ]; then step1_pass=$((step1_pass+1)); else echo "MISS $id -> $v"; fi
    else
        echo "TRANSPORT-FAIL $id: $GRADE_ERR" >&2; fail=1; step1_err=$((step1_err+1))
    fi
done <"$ATOMS"
echo "step1: $step1_pass/$step1_total PASS (transport-errors=$step1_err)"

echo "=== STEP 2: paraphrases ($PRIMARY_MODEL) ==="
s2_pass=0; s2_total=0
while IFS= read -r line; do
    pid="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    iid="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['item_id'])")"
    ans="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['answer'])")"
    base="$(grep -F "\"id\": \"$iid\"" "$ATOMS" | head -1)"
    if [ -z "$base" ]; then base="$(python3 - "$ATOMS" "$iid" <<'EOF'
import json,sys
for line in open(sys.argv[1]):
    d=json.loads(line)
    if d['id']==sys.argv[2]: print(json.dumps(d)); break
EOF
)"; fi
    q="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    aj="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['atoms']))")"
    fj="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['forbidden']))")"
    ab="$(printf '%s' "$base" | python3 -c "import json,sys; print('yes' if json.load(sys.stdin)['abstention_required'] else 'no')")"
    sp="$(printf '%s' "$base" | python3 -c "import json,sys; print(json.load(sys.stdin)['spans'])")"
    if out="$(grade_one "$PRIMARY_MODEL" "$pid" "paraphrase" "$q" "$aj" "$fj" "$ab" "$ans" "$sp")"; then
        printf '%s\n' "$out" >>"$LEDGER"
        v="$(printf '%s' "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); print(str(d['judge']['correct']))")"
        s2_total=$((s2_total+1))
        [ "$v" = "True" ] && s2_pass=$((s2_pass+1))
        echo "$pid($iid) -> $v"
    else
        echo "TRANSPORT-FAIL $pid: $GRADE_ERR" >&2; fail=1
    fi
done <"$PARAS"
echo "step2: $s2_pass/$s2_total PASS"

echo "=== STEP 3: adversarial (vague|forbidden|markerless, $PRIMARY_MODEL) ==="
s3_vrej=0; s3_vtot=0; s3_frej=0; s3_ftot=0; s3_mrej=0; s3_mtot=0
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
        if out="$(grade_one "$PRIMARY_MODEL" "$id" "adv-$short" "$q" "$aj" "$fj" "$ab" "$ans" "$sp")"; then
            printf '%s\n' "$out" >>"$LEDGER"
            v="$(printf '%s' "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); print(str(d['judge']['correct']))")"
            case "$short" in
                vague) s3_vtot=$((s3_vtot+1)); [ "$v" = "False" ] && s3_vrej=$((s3_vrej+1)) || echo "VAGUE-PASS $id";;
                forbidden) s3_ftot=$((s3_ftot+1)); [ "$v" = "False" ] && s3_frej=$((s3_frej+1)) || echo "FORB-PASS $id";;
                markerless) s3_mtot=$((s3_mtot+1)); [ "$v" = "False" ] && s3_mrej=$((s3_mrej+1)) || echo "MARK-PASS $id";;
            esac
        else
            echo "TRANSPORT-FAIL $id/$short: $GRADE_ERR" >&2; fail=1
        fi
    done
done <"$ATOMS"
echo "step3 vague-reject: $s3_vrej/$s3_vtot | forbidden-reject: $s3_frej/$s3_ftot | markerless-reject: $s3_mrej/$s3_mtot"

# --- costs ---
read -r TOT_TOK TOT_DOL NREC <<<"$(python3 - "$LEDGER" <<'EOF'
import json,sys
t=d=n=0
for line in open(sys.argv[1]):
    r=json.loads(line)
    if r.get('record')=='judge': t+=r['tokens']; d+=r['dollars']; n+=1
print(t, f"{d:.6f}", n)
EOF
)"
echo "cost: tokens=$TOT_TOK dollars=$TOT_DOL records=$NREC"
echo "LEDGER=$LEDGER"
# summary line appended by caller step 4/5 wrapper; emit machine summary
python3 - "$TS" "$PRIMARY_MODEL" "$step1_pass" "$step1_total" "$step1_err" "$s2_pass" "$s2_total" "$s3_vrej" "$s3_vtot" "$s3_frej" "$s3_ftot" "$s3_mrej" "$s3_mtot" "$TOT_TOK" "$TOT_DOL" "$NREC" <<'EOF'
import json,sys
(ts,model,s1p,s1t,s1e,s2p,s2t,vrej,vtot,frej,ftot,mrej,mtot,tok,dol,n)=sys.argv[1:17]
print(json.dumps({"record":"judge-summary","ts":ts,"model":model,
 "step1_pass":int(s1p),"step1_total":int(s1t),"step1_transport_errors":int(s1e),
 "step2_pass":int(s2p),"step2_total":int(s2t),
 "vague_reject":int(vrej),"vague_total":int(vtot),
 "forbidden_reject":int(frej),"forbidden_total":int(ftot),
 "markerless_reject":int(mrej),"markerless_total":int(mtot),
 "tokens":int(tok),"dollars":float(dol),"records":int(n)}))
EOF
exit $fail
