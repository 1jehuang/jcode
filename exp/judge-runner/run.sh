#!/bin/bash
# run.sh -- shared blind-judge runner: transport + blind-grade + ledger core.
# Serves d1-P2 (--head=h1 six-dim probe grading), d2-P1/P2/P3 (H2/H4/H5 over
# listwise ledgers), d3-P5 (H3 is retrieval-side; the core rescores its
# swap/DiRe ablations through the same blind path).
# Extends the harness/judge-calibrate.sh transport pattern (proxy-direct POST
# to PROXY_BASE/chat/completions, pinned model id, per-call timeout, primary
# -> ONE backup -> NEEDS-WORK, exit 0/2). It does NOT refactor that file;
# harness/run.sh, pin-five.json and Tier-1 gates are untouched.
#
# Blind discipline: items may carry `arm` (e.g. compacted-vs-full, base-vs-
# swap). The arm is STRIPPED before prompt construction and recorded only in
# the ledger `arm` field (unblinded in head analysis, never at grade time).
# Dev-fixture items only; frozen 03-heldout/04-multihop sets are never opened
# here (no path to them exists in this kit).
#
# Fail-closed: after 2 attempts any `error != null` becomes a NEEDS-WORK
# ledger row (verdict nulls), never a guessed grade.
# Error taxonomy: null | parse | credential | timeout | rate_limit | transport.
#
# Usage:
#   exp/judge-runner/run.sh --head=H --input=ITEMS.jsonl --out=LEDGER.jsonl \
#       --primary=MODEL [--backup=MODEL] [--temperature=T] [--timeout_s=N]
#   --head: listwise (default; {qid,query,candidates[]} -> relevant_ids) or
#           h1 (six-dim probe grading; {qid,probe_type,probe_question,answer}).
# Env: OPENAI_COMPAT_API_KEY (or ~/.config/jcode/opencode-proxy.env),
#      PROXY_BASE (default http://127.0.0.1:8787/v1),
#      PRICE_PER_1K (default 0.002).
# Exit: 0 = ledger written (check NEEDS-WORK summary); 2 = transport failure
#       (missing credential, mid-run 401/403, or any timeout/rate_limit/
#       transport NEEDS-WORK row).
#
# Model ids are CLI flags, never hardcoded: --primary is REQUIRED. Re-verify
# pinned ids per d2-P0(a) before any spend (ids drift since 17-judge-calib).

set -u

HEAD="listwise"
INPUT=""
OUT=""
PRIMARY=""
BACKUP=""
TEMPERATURE="0"
TIMEOUT_S="12"
PROXY_BASE="${PROXY_BASE:-http://127.0.0.1:8787/v1}"
PRICE_PER_1K="${PRICE_PER_1K:-0.002}"

for a in "$@"; do
    case "$a" in
        --head=*) HEAD="${a#--head=}" ;;
        --input=*) INPUT="${a#--input=}" ;;
        --out=*) OUT="${a#--out=}" ;;
        --primary=*) PRIMARY="${a#--primary=}" ;;
        --backup=*) BACKUP="${a#--backup=}" ;;
        --temperature=*) TEMPERATURE="${a#--temperature=}" ;;
        --timeout_s=*) TIMEOUT_S="${a#--timeout_s=}" ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "unknown arg: $a" >&2; exit 2 ;;
    esac
done

[ -n "$INPUT" ] || { echo "missing --input" >&2; exit 2; }
[ -n "$OUT" ] || { echo "missing --out" >&2; exit 2; }
[ -n "$PRIMARY" ] || { echo "missing --primary (model ids are CLI flags; re-verify per d2-P0(a))" >&2; exit 2; }
[ "$HEAD" = "listwise" ] || [ "$HEAD" = "h1" ] || { echo "unknown --head=$HEAD (listwise|h1)" >&2; exit 2; }
[ -f "$INPUT" ] || { echo "input not found: $INPUT" >&2; exit 2; }

if [ -z "${OPENAI_COMPAT_API_KEY:-}" ] && [ -f "$HOME/.config/jcode/opencode-proxy.env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.config/jcode/opencode-proxy.env"
fi
if [ -z "${OPENAI_COMPAT_API_KEY:-}" ]; then
    echo "TRANSPORT-ERROR: no OPENAI_COMPAT_API_KEY (proxy server key missing)" >&2
    exit 2
fi

# Listwise system prompt: strict precision-first, JSON-array-only. Mirrors
# src/bin/memory_recall_bench.rs JUDGE_SYSTEM so bench --ledger rows join here.
LISTWISE_SYSTEM='You judge whether stored MEMORIES would be genuinely useful to surface to an AI coding agent given the CURRENT conversation context. Be strict and prefer precision: a memory is relevant ONLY if a competent engineer would say "yes, knowing this specifically helps respond here." Mark relevant when the memory is a fact, user preference, correction, or procedure that applies to what is happening right now. Mark NOT relevant when it is off-topic, generic/obvious, only shares surface keywords, or would be noise. When unsure, exclude it. The context contains boilerplate (system reminders, tool output); focus on what is actually being worked on. Reply with ONLY a JSON array of the relevant candidate numbers, e.g. [1,4] or []. No prose.'

# H1 six-dim probe prompt (d1-P2 compaction probes).
H1_SYSTEM='You grade ONE probe answer about a memory-system context on six dimensions. Score each dimension 0 (fail) or 1 (pass): accuracy, context_awareness, artifact_trail, completeness, continuity, instruction_following. Reply with ONLY a JSON object like {"accuracy":1,"context_awareness":0,"artifact_trail":1,"completeness":1,"continuity":0,"instruction_following":1}. No prose.'

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

# build_req <item.json> <model>: prompt + request body + meta (qid/arm/step/n/ids).
build_req() {
    HEAD="$HEAD" ITEM="$1" MODEL="$2" TEMP="$TEMPERATURE" TMPD="$TMPD" \
    LWS="$LISTWISE_SYSTEM" H1S="$H1_SYSTEM" python3 <<'EOF'
import json, os
item = json.load(open(os.environ["ITEM"]))
head = os.environ["HEAD"]
tmpd = os.environ["TMPD"]
qid = item["qid"]  # KeyError -> bad item, caller skips
arm = item.get("arm", "base")
step = item.get("step", "judge")
if head == "h1":
    system = os.environ["H1S"]
    user = ("PROBE TYPE: %s\nPROBE QUESTION: %s\nCANDIDATE ANSWER: %s\n\n"
            "Score the answer 0/1 on accuracy, context_awareness, "
            "artifact_trail, completeness, continuity, instruction_following. "
            "JSON object only." % (item.get("probe_type", ""),
            item.get("probe_question", ""), item.get("answer", "")))
    ids: list = []
else:
    system = os.environ["LWS"]
    # BLIND: only query + candidate content cross into the prompt. `arm`
    # and every other item field stay out.
    query = item["query"]
    if len(query) > 6000:  # keep TAIL (mirrors bench truncate_for_judge)
        query = query[-6000:]
    lines = ["CURRENT CONTEXT:", query, "", "CANDIDATE MEMORIES:"]
    ids = []
    for i, c in enumerate(item["candidates"]):
        lines.append("%d. %s" % (i + 1, c["content"].replace("\n", " ")))
        ids.append(c["id"])
    lines += ["", "Return the numbers of the relevant memories as a JSON array."]
    user = "\n".join(lines)
req = {"model": os.environ["MODEL"],
       "messages": [{"role": "system", "content": system},
                    {"role": "user", "content": user}],
       "temperature": float(os.environ["TEMP"]), "max_tokens": 512}
json.dump(req, open(tmpd + "/req.json", "w"))
open(tmpd + "/prompt.txt", "w").write(system + "\n" + user)
json.dump({"qid": qid, "arm": arm, "step": step, "ids": ids},
          open(tmpd + "/meta.json", "w"))
EOF
}

# parse_resp <raw-body-file>: stdout = ledger line (error null OR parse).
# Sets PARSE_ERR detail on verdict failure (caller retries once, then
# NEEDS-WORK -- parse is never graded fail-open).
PARSE_ERR=""
parse_resp() {
    local line
    line="$(HEAD="$HEAD" MODEL="$GRADE_MODEL" ATTEMPT="$GRADE_ATTEMPT" \
        PRICE="$PRICE_PER_1K" BODY="$1" PROMPT="$TMPD/prompt.txt" \
        META="$TMPD/meta.json" python3 <<'EOF'
import json, os
raw = open(os.environ["BODY"]).read()
prompt = open(os.environ["PROMPT"]).read()
meta = json.load(open(os.environ["META"]))
head = os.environ["HEAD"]
model = os.environ["MODEL"]
attempt = int(os.environ["ATTEMPT"])
price = float(os.environ["PRICE"])
qid, arm, step, ids = meta["qid"], meta["arm"], meta["step"], meta["ids"]
verdict = None
derr = None
try:
    d = json.loads(raw)
    m = d["choices"][0]["message"]
    content = m.get("content") or ""
    if not content.strip():
        content = m.get("reasoning_content") or ""
    assert content.strip(), "empty-content"
except Exception as e:
    content = ""
    derr = "transport:bad-envelope:" + str(e)[:60]
if derr is None:
    try:
        if head == "h1":
            s, e = content.find("{"), content.rfind("}")
            assert s >= 0 and e > s, "no-json-object"
            v = json.loads(content[s:e + 1])
            dims = ["accuracy", "context_awareness", "artifact_trail",
                    "completeness", "continuity", "instruction_following"]
            scored = {}
            for k in dims:
                x = v[k]
                assert isinstance(x, (int, bool)) and int(x) in (0, 1), k + "-not-01"
                scored[k] = int(x)
            verdict = {"dims": scored}
        else:
            s, e = content.find("["), content.rfind("]")
            assert s >= 0 and e > s, "no-json-array"
            nums = json.loads(content[s:e + 1])
            idx = [int(x) - 1 for x in nums
                   if isinstance(x, int) and 1 <= int(x) <= len(ids)]
            verdict = {"relevant_ids": [ids[i] for i in idx]}
    except Exception as e:
        derr = "parse:" + str(e)[:80]
tok = (len(prompt) + len(raw) + 3) // 4
dol = round(tok * price / 1000, 6)
err = None
if derr is not None:
    err = derr.split(":")[0]  # taxonomy class; detail stays in judge.error
    if err not in ("parse", "credential", "timeout", "rate_limit", "transport"):
        err = "transport"
print(json.dumps({"record": "judge", "qid": qid, "head": head, "arm": arm,
  "step": step, "model": model, "attempt": attempt,
  "judge": {"verdict": verdict, "error": derr, "model": model},
  "em_correct": None, "agree": None, "abstained": None, "swap_agree": None,
  "votes": None, "unanimous": None, "category": None,
  "tokens": tok, "dollars": dol, "error": err}))
EOF
)"
    local rc=$?
    [ $rc -eq 0 ] || { PARSE_ERR="transport:ledger-encode-failed"; return 1; }
    printf '%s\n' "$line" >"$TMPD/line.json"
    PARSE_ERR="$(python3 -c "import json,sys; d=json.load(open('$TMPD/line.json')); print(d['judge']['error'] or '')")"
    [ -z "$PARSE_ERR" ]  # 0 = clean verdict, 1 = parse/transport detail
}

# needswork_row <ERR-class> <detail>: stdout ledger line with verdict nulls.
needswork_row() {
    MODEL="$GRADE_MODEL" ATTEMPT="$GRADE_ATTEMPT" PRICE="$PRICE_PER_1K" \
    ERR="$1" DETAIL="$2" PROMPT="$TMPD/prompt.txt" META="$TMPD/meta.json" \
    HEAD="$HEAD" python3 <<'EOF'
import json, os
prompt = open(os.environ["PROMPT"]).read()
meta = json.load(open(os.environ["META"]))
tok = (len(prompt) + 3) // 4
dol = round(tok * float(os.environ["PRICE"]) / 1000, 6)
print(json.dumps({"record": "judge", "qid": meta["qid"], "head": os.environ["HEAD"],
  "arm": meta["arm"], "step": meta["step"], "model": os.environ["MODEL"],
  "attempt": int(os.environ["ATTEMPT"]),
  "judge": {"verdict": None, "error": os.environ["DETAIL"], "model": os.environ["MODEL"]},
  "em_correct": None, "agree": None, "abstained": None, "swap_agree": None,
  "votes": None, "unanimous": None, "category": None,
  "tokens": tok, "dollars": dol, "error": os.environ["ERR"]}))
EOF
}

TS="$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$(dirname "$OUT")"
python3 - "$TS" "$PRIMARY" "$BACKUP" "$HEAD" <<'EOF' >"$OUT"
import json, sys, subprocess
ts, primary, backup, head = sys.argv[1:5]
try:
    sha = subprocess.check_output(["git", "rev-parse", "--short", "HEAD"],
                                  text=True).strip()
except Exception:
    sha = "unknown"
print(json.dumps({"record": "judge-run", "started_at": ts, "repo_sha": sha,
  "primary_model": primary, "backup_model": backup or None, "head": head,
  "transport": "proxy-direct", "openrouter": "excluded",
  "blind": "arm-stripped-at-grade", "fail_closed": "error-after-2-is-needswork"}))
EOF

n_total=0; n_ok=0; n_nw=0; nw_qids=""; transport_fail=0; n_skip=0
while IFS= read -r line || [ -n "$line" ]; do
    [ -z "$line" ] && continue
    printf '%s' "$line" >"$TMPD/item.json"
    qid="$(python3 -c "import json; print(json.load(open('$TMPD/item.json')).get('qid','?'))" 2>/dev/null || echo "?")"
    if ! build_req "$TMPD/item.json" "$PRIMARY" 2>"$TMPD/build.err"; then
        echo "SKIP $qid: bad item shape ($(head -c 120 "$TMPD/build.err"))" >&2
        n_skip=$((n_skip+1)); continue
    fi
    n_total=$((n_total+1))
    GRADE_ATTEMPT=0; GRADE_MODEL="$PRIMARY"; Grade_err=""; done_item=0
    while [ $GRADE_ATTEMPT -lt 2 ] && [ $done_item -eq 0 ]; do
        GRADE_ATTEMPT=$((GRADE_ATTEMPT+1))
        if [ $GRADE_ATTEMPT -eq 2 ] && [ -n "$BACKUP" ]; then
            GRADE_MODEL="$BACKUP"
            build_req "$TMPD/item.json" "$GRADE_MODEL"
        fi
        if ! http="$(curl -s -m "$TIMEOUT_S" -w '\n%{http_code}' \
                "$PROXY_BASE/chat/completions" \
                -H "Authorization: Bearer $OPENAI_COMPAT_API_KEY" \
                -H "Content-Type: application/json" -d @"$TMPD/req.json" \
                2>"$TMPD/curl.err")"; then
            rc=$?
            if [ $rc -eq 28 ]; then Grade_err="timeout"; Grade_detail="timeout:curl-28-after-${TIMEOUT_S}s";
            else Grade_err="transport"; Grade_detail="transport:curl-rc=$rc:$(head -c 120 "$TMPD/curl.err")"; fi
            sleep 1; continue
        fi
        code="$(printf '%s' "$http" | tail -1)"
        printf '%s' "$http" | sed '$d' >"$TMPD/body.txt"
        case "$code" in
            200) if parse_resp "$TMPD/body.txt"; then
                     done_item=1
                 else
                     Grade_err="$(printf '%s' "$PARSE_ERR" | cut -d: -f1)"
                     Grade_detail="$PARSE_ERR"
                     # parse = NEEDS-WORK after 1 retry: loop continues once.
                 fi ;;
            429) Grade_err="rate_limit"; Grade_detail="rate_limit:http-429:$(head -c 120 "$TMPD/body.txt")"; sleep 2 ;;
            401|403) echo "CREDENTIAL-ERROR $qid: http-$code (Tier-1-only offline; partial ledger kept)" >&2
                 needswork_row "credential" "credential:http-$code" >>"$OUT"
                 n_nw=$((n_nw+1)); nw_qids="$nw_qids $qid"; transport_fail=1
                 echo "LEDGER=$OUT (aborted: credential)" >&2; exit 2 ;;
            *) Grade_err="transport"; Grade_detail="transport:http-$code:$(head -c 120 "$TMPD/body.txt")"; sleep 1 ;;
        esac
    done
    if [ $done_item -eq 1 ]; then
        cat "$TMPD/line.json" >>"$OUT"; n_ok=$((n_ok+1))
    else
        needswork_row "$Grade_err" "$Grade_detail" >>"$OUT"
        n_nw=$((n_nw+1)); nw_qids="$nw_qids $qid"
        case "$Grade_err" in timeout|rate_limit|transport|credential) transport_fail=1 ;; esac
        echo "NEEDS-WORK $qid: $Grade_detail" >&2
    fi
done <"$INPUT"

echo "graded=$n_total ok=$n_ok needswork=$n_nw skipped_bad_shape=$n_skip"
[ -n "$nw_qids" ] && echo "NEEDS-WORK qids:$nw_qids"
echo "LEDGER=$OUT"
[ $transport_fail -eq 1 ] && exit 2
exit 0
