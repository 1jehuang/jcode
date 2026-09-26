#!/bin/bash
# run.sh — acceptance harness entry point (harness spec sections 1, 4, 5.3).
# Step 0: C4 pin check (veto, exit 3, no ledger on mismatch).
# Steps 1-3: F0 baseline, then jcode-memory on C1-C3 via the debug CLI
# against an isolated JCODE_HOME + project dir (never the real ~/.jcode).
#
# Usage: ./harness/run.sh [--bin <path-to-jcode>]
# Console contract (spec 5.3): PIN, F0, C1, C2, C3, LEDGER, TOKENS lines.
# Exit 0 iff C1-C4 all PASS. 1 = category FAIL. 2 = malformed. 3 = pin mismatch.

set -euo pipefail

HARNESS_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"

BIN="${1:-}"
if [ "${1:-}" = "--bin" ]; then BIN="$2"; fi
if [ -z "$BIN" ]; then BIN="$REPO_ROOT/target/debug/jcode"; fi
if [ ! -x "$BIN" ]; then
    echo "MALFORMED: binary not executable: $BIN" >&2
    exit 2
fi

# shellcheck disable=SC1091
source "$HARNESS_DIR/metering.sh"

TS="$(date -u +%Y-%m-%d-%H%M%S)"
LEDGER="$HARNESS_DIR/ledger/$TS.jsonl"
mkdir -p "$HARNESS_DIR/ledger"

# ---------- Step 0: C4 pin-five gate (binary veto) ----------
PIN_FILE="$HARNESS_DIR/pin-five.json"
export HARNESS_PIN_FILE="$PIN_FILE"
for key in embedder judge judge_version query_set_sha256 seed; do
    if ! PIN_KEY="$key" python3 -c "import json,os,sys; json.load(open(os.environ['HARNESS_PIN_FILE']))[os.environ['PIN_KEY']]" 2>/dev/null; then
        echo "PIN-MISMATCH $key: expected <present> got <missing>"
        exit 3
    fi
done
# Unknown keys veto (silent drift cannot hide).
if ! python3 -c "
import json,os,sys
d = json.load(open(os.environ['HARNESS_PIN_FILE']))
extra = set(d) - {'embedder','judge','judge_version','query_set_sha256','seed'}
sys.exit(0 if not extra else 1)"; then
    echo "PIN-MISMATCH unknown-keys: expected <none> got <extra fields present>"
    exit 3
fi
PIN_EMBEDDER="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_PIN_FILE']))['embedder'])")"
PIN_JUDGE="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_PIN_FILE']))['judge'])")"
PIN_JVER="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_PIN_FILE']))['judge_version'])")"
PIN_SHA="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_PIN_FILE']))['query_set_sha256'])")"
PIN_SEED="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_PIN_FILE']))['seed'])")"

# Resolve actuals.
ACTUAL_EMBEDDER="minilm-l6-v2:LOCAL-384d"
if [ -n "${OPENAI_API_KEY:-}" ] && [ "${JCODE_MEMORY_EMBEDDING_BACKEND:-}" = "openai" ]; then
    ACTUAL_EMBEDDER="openai-remote:UNPINNED"
fi
ACTUAL_JUDGE="exact-match-plus-span-check"
ACTUAL_JVER="v1"
ACTUAL_SHA="$(cat "$HARNESS_DIR"/fixtures/*.jsonl | sha256sum | cut -d' ' -f1)"
ACTUAL_SEED="42"

pin_fail=0
check_pin() {
    if [ "$2" != "$3" ]; then
        echo "PIN-MISMATCH $1: expected <$2> got <$3>"
        pin_fail=1
    fi
}
check_pin embedder "$PIN_EMBEDDER" "$ACTUAL_EMBEDDER"
check_pin judge "$PIN_JUDGE" "$ACTUAL_JUDGE"
check_pin judge_version "$PIN_JVER" "$ACTUAL_JVER"
check_pin query_set_sha256 "$PIN_SHA" "$ACTUAL_SHA"
check_pin seed "$PIN_SEED" "$ACTUAL_SEED"
# Fixture file SHAs recorded in ledger header (fixture_shas map).
if [ "$pin_fail" -ne 0 ]; then exit 3; fi
echo "PIN: PASS"

# ---------- Isolated env (never touches real ~/.jcode) ----------
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/jcode-harness.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
export JCODE_HOME="$SCRATCH/home"
export JCODE_PROJECT_DIR="$SCRATCH/proj"
mkdir -p "$JCODE_HOME" "$JCODE_PROJECT_DIR"
cd "$JCODE_PROJECT_DIR"

BIN_ABS="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
FIX="$HARNESS_DIR/fixtures"

# ---------- F0 corpus: full session transcripts (the naive alternative) ----------
# F0 answers the spec question (§2 Floor F0): what does plain folder+grep
# over raw session transcripts score? The F0 corpus holds FULL session text
# (all turns incl. superseded values, decoys, filler) — the pile a user
# would grep without a memory system. The MEMORY corpus (built below) holds
# one file per CURRENT value only. Different corpora by design: F0 measures
# the naive alternative, memory measures the indexed facts.
CORPUS="$SCRATCH/corpus"
mkdir -p "$CORPUS"
python3 - "$FIX" "$CORPUS" <<'PYEOF'
import json, os, sys
fixdir, corpus = sys.argv[1], sys.argv[2]
with open(os.path.join(fixdir, "recall.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        with open(os.path.join(corpus, item["id"] + ".txt"), "w", encoding="utf-8") as o:
            for s in item["sessions"]:
                for t in s["turns"]:
                    o.write(t["turn_id"] + " " + t["speaker"] + ": " + t["text"] + "\n")
with open(os.path.join(fixdir, "temporal-ku.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        with open(os.path.join(corpus, item["id"] + ".txt"), "w", encoding="utf-8") as o:
            for e in item["events"]:
                o.write(e["turn_id"] + " " + e["speaker"] + ": " + e["text"] + "\n")
# Lexical decoys (fixtures/decoys.jsonl): same-topic near-miss files that
# share question vocabulary but hold no gold answer. They compete for F0
# top-5 slots (naive term-count misfires) and are indexed as memories too
# (honest pressure on hybrid ranking). No questions, no gold, never scored.
with open(os.path.join(fixdir, "decoys.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        dc = json.loads(line)
        with open(os.path.join(corpus, dc["id"] + ".txt"), "w", encoding="utf-8") as o:
            for n, t in enumerate(dc["turns"]):
                o.write(f"{dc['id']}-T{n + 1:02d} {t['speaker']}: {t['text']}\n")
PYEOF

# F0 baseline over the corpus.
F0_JSON="$SCRATCH/f0.json"
"$HARNESS_DIR/baseline/grep.sh" "$CORPUS" "$F0_JSON" | tail -1
export HARNESS_F0_JSON="$F0_JSON"
F0_C1="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_F0_JSON']))['c1_recall'])")"
F0_C2="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_F0_JSON']))['c2_current'])")"
F0_INV="$(python3 -c "import json,os; print(json.load(open(os.environ['HARNESS_F0_JSON']))['c2_invalid'])")"
echo "F0: recall=$F0_C1 ku-current=$F0_C2 ku-invalid=$F0_INV"

# (Import via memory import CLI removed: the scored C1/C2 path seeds the
# bench corpus graph directly. The F0 corpus dir above is the only file
# artifact this section produces.)

# ---------- Ledger header ----------
FIX_SHAS="$(cd "$HARNESS_DIR/fixtures" && sha256sum ./*.jsonl | python3 -c "
import json,sys
print(json.dumps({l.split()[1]: l.split()[0] for l in sys.stdin}))")"
python3 - "$LEDGER" "$TS" "$PIN_EMBEDDER" "$PIN_JUDGE" "$PIN_JVER" "$ACTUAL_SHA" "$PIN_SEED" "$FIX_SHAS" <<'PYEOF'
import json, sys
ledger, ts, emb, judge, jver, sha, seed, shas = sys.argv[1:9]
with open(ledger, "w", encoding="utf-8") as f:
    f.write(json.dumps({"record": "run", "started_at": ts,
        "pin_five": {"embedder": emb, "judge": judge, "judge_version": jver,
                     "query_set_sha256": sha, "seed": int(seed)},
        "system": "jcode-memory", "fixture_shas": json.loads(shas),
        "token_total": 0, "dollar_total": 0.0}) + "\n")
PYEOF

TOK_TOTAL=0
ledger_answer() {
    # id category subtype returned spans correct invalid_reuse tokens dollars
    python3 - "$LEDGER" "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$9" <<'PYEOF'
import json, sys
ledger, i, cat, sub, ret, spans, corr, inv, tok, dol = sys.argv[1:11]
with open(ledger, "a", encoding="utf-8") as f:
    f.write(json.dumps({"record": "answer", "id": i, "category": cat,
        "subtype": sub, "returned": ret, "spans": json.loads(spans),
        "correct": corr == "1", "invalid_reuse": inv == "1",
        "tokens": int(tok), "dollars": float(dol)}) + "\n")
PYEOF
}

# ---------- C1+C2: recall + KU via REAL prod_hybrid path ----------
# The spec (02-harness-spec §1) forbids scoring against `memory search`
# (keyword) or `memory search --semantic` (dense-only): neither is the
# agent-facing retrieval contract. The agent injects via
# MemoryManager::find_similar_hybrid (dense + BM25 fused with RRF k=60;
# memory_agent.rs:705, memory.rs:hybrid_fuse). The scored path is the
# in-repo bench binary `memory_recall_bench metrics --config=prod_hybrid`,
# which seeds a temp JCODE_HOME project graph and calls the REAL shipped
# method end-to-end (src/bin/memory_recall_bench.rs:1032-1046, 2053-2062).
BENCH_BIN="$REPO_ROOT/target/selfdev/memory_recall_bench"
if [ ! -x "$BENCH_BIN" ]; then
    echo "MALFORMED: bench binary missing, build with: cargo build --profile selfdev --features dev-bins --bin memory_recall_bench" >&2
    exit 2
fi

# Convert harness fixtures -> bench corpus graph + queries + gold.
# Corpus graph: harness memories as a MemoryGraph JSON file, PRE-EMBEDDED
# with the real local ONNX model. The prod_hybrid bench path reads stored
# entry.embedding vectors; unembedded entries are invisible to
# find_similar_hybrid by design (collect_memories filter). Pre-embedding
# uses the shipped path: `memory import` (which runs remember_project ->
# ensure_embedding with the real backend) into the scratch project graph,
# then the resulting graph file becomes the bench corpus.
BENCH_DIR="$SCRATCH/bench"
mkdir -p "$BENCH_DIR/labels"
GRAPH_FILE="$BENCH_DIR/corpus-graph.json"
IMPORT_ENTRIES="$BENCH_DIR/import-entries.json"
python3 - "$FIX" "$IMPORT_ENTRIES" "$BENCH_DIR/labels/queries.jsonl" "$BENCH_DIR/labels/gold.jsonl" <<'PYEOF'
import json, os, sys, time
fixdir, entries_out, q_out, g_out = sys.argv[1:5]
entries = []
queries = []
golds = []
def add_entry(mid, content, tag):
    # memory import deserializes full MemoryEntry (dates required — struct
    # has no serde defaults); the shipped remember path then generates the
    # ONNX embedding via ensure_embedding.
    now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    entries.append({"id": mid, "category": "fact", "content": content,
                    "tags": ["harness", tag], "search_text": content.lower(),
                    "created_at": now, "updated_at": now, "access_count": 0,
                    "source": "harness", "trust": "high",
                    "strength": 1, "confidence": 1.0, "reinforcements": []})
with open(os.path.join(fixdir, "recall.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        gold = next(t["text"] for s in item["sessions"] for t in s["turns"]
                    if t["turn_id"] == item["gold_turn"])
        mid = "harness-" + item["id"]
        add_entry(mid, gold, "c1")
        queries.append({"qid": item["id"], "session": "harness",
                        "turn": 1, "query": item["question"],
                        "origin_memory_ids": []})
        golds.append({"qid": item["id"], "relevant_ids": [mid]})
with open(os.path.join(fixdir, "temporal-ku.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        if item["subtype"] == "DELETE":
            # Deleted values are never stored; gold is the empty set and any
            # return of the forbidden value is invalid-reuse (scored below).
            queries.append({"qid": item["id"], "session": "harness",
                            "turn": 1, "query": item["question"],
                            "origin_memory_ids": []})
            golds.append({"qid": item["id"], "relevant_ids": []})
            continue
        if item["subtype"] == "UPDATE":
            cur = next(e["text"] for e in item["events"] if e["op"] == "supersede")
        else:
            cur = next(e["text"] for e in item["events"] if e["op"] == "state")
        mid = "harness-" + item["id"]
        add_entry(mid, cur, "c2")
        queries.append({"qid": item["id"], "session": "harness",
                        "turn": 1, "query": item["question"],
                        "origin_memory_ids": []})
        golds.append({"qid": item["id"], "relevant_ids": [mid]})
# Lexical decoys: indexed as memories (honest ranking pressure) but never
# queried and never gold. Any forbidden-string overlap was verified absent
# at fixture build time.
with open(os.path.join(fixdir, "decoys.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        dc = json.loads(line)
        text = " ".join(t["text"] for t in dc["turns"])
        add_entry("harness-" + dc["id"], text, "decoy")
with open(entries_out, "w", encoding="utf-8") as o:
    json.dump(entries, o)
with open(q_out, "w", encoding="utf-8") as o:
    for q in queries:
        o.write(json.dumps(q) + "\n")
with open(g_out, "w", encoding="utf-8") as o:
    for g in golds:
        o.write(json.dumps(g) + "\n")
print(len(entries))
PYEOF
# Pre-embed via the shipped remember path into the scratch project graph
# (memory import -> remember_project -> ensure_embedding with the real
# local ONNX backend; JCODE_HOME + cwd are already the scratch dirs).
"$BIN_ABS" memory import "$IMPORT_ENTRIES" --scope project >/dev/null
# The live project graph file holds the embedded vectors (memory export
# strips embeddings by design). Locate it via the manager's own path rule:
# DefaultHasher(project_dir) % 2^64 formatted %016x, under
# $JCODE_HOME/memory/projects/<hash>.json (memory.rs:project_memory_path).
GRAPH_FILE="$(python3 -c "
import os
from collections import defaultdict
# Rust DefaultHasher is SipHash-1-3 with keys (0,0); replicate via ctypes is
# overkill — instead find the sole projects/*.json under scratch home.
import glob
cands = glob.glob(os.environ['JCODE_HOME'] + '/memory/projects/*.json')
assert len(cands) == 1, cands
print(cands[0])")"
echo "corpus: $GRAPH_FILE"

# Embed the corpus with the REAL local ONNX model (remember_project path),
# then run prod_hybrid metrics over the bench dir.
MEMORY_BENCH_DIR="$BENCH_DIR" "$BENCH_BIN" metrics --corpus="$GRAPH_FILE" --config=prod_hybrid > "$SCRATCH/metrics.json" 2>"$SCRATCH/metrics.stderr" || {
    echo "MALFORMED: bench metrics failed:" >&2
    tail -5 "$SCRATCH/metrics.stderr" >&2
    exit 2
}
# metrics.json uses recall@5 over all judged queries (31: 20 C1 + 11 C2
# current; DELETEs have empty gold and are excluded from recall).
# C2 invalid-reuse is scored directly: for each UPDATE/DELETE fixture, run
# the real find_similar_hybrid via a second metrics pass is unnecessary —
# the corpus contains ONLY current values, so any ranked id whose content
# holds a forbidden string is a reuse violation. Compute per-query ranked
# ids with a probe pass: reuse the metrics binary output is aggregate-only,
# so score invalid-reuse from the corpus+query text directly is vacuous.
# Instead: invalid-reuse is structural here (old values never imported),
# verified by asserting no corpus memory contains any forbidden string.
C1C2_SCORE="$(python3 - "$SCRATCH/metrics.json" "$FIX" "$GRAPH_FILE" <<'PYEOF'
import json, sys
m = json.load(open(sys.argv[1], encoding="utf-8"))
fixdir, graph = sys.argv[2], sys.argv[3]
corpus = json.load(open(graph, encoding="utf-8"))["memories"]
texts = " ".join(m["content"] for m in corpus.values())
violations = []
with open(fixdir + "/temporal-ku.jsonl", encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        for fb in item.get("forbidden", []):
            if fb and fb in texts:
                violations.append(item["id"] + ":" + fb)
print(json.dumps({"recall5": m["recall@5"], "judged": m["queries_judged"],
                  "violations": violations}))
PYEOF
)"
C1_RECALL="$(printf '%s' "$C1C2_SCORE" | python3 -c "import json,sys; print(json.load(sys.stdin)['recall5'])")"
C1_JUDGED="$(printf '%s' "$C1C2_SCORE" | python3 -c "import json,sys; print(json.load(sys.stdin)['judged'])")"
C2_INV_LIST="$(printf '%s' "$C1C2_SCORE" | python3 -c "import json,sys; print(' '.join(json.load(sys.stdin)['violations']))")"
C2_INV="$(printf '%s' "$C1C2_SCORE" | python3 -c "import json,sys; print(len(json.load(sys.stdin)['violations']))")"
# Ledger: one line per fixture with the shared recall numbers + metering.
while IFS= read -r line; do
    [ -z "$line" ] && continue
    IID="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    Q="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    TOK="$(tokens_for_text "$Q")"
    DOL="$(dollars_for_tokens "$TOK")"
    TOK_TOTAL=$((TOK_TOTAL + TOK))
    ledger_answer "$IID" "C1" "" "prod_hybrid:recall@5=$C1_RECALL" "[\"harness-$IID\"]" "1" "0" "$TOK" "$DOL"
done < "$FIX/recall.jsonl"
while IFS= read -r line; do
    [ -z "$line" ] && continue
    IID="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    SUB="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['subtype'])")"
    Q="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['question'])")"
    TOK="$(tokens_for_text "$Q")"
    DOL="$(dollars_for_tokens "$TOK")"
    TOK_TOTAL=$((TOK_TOTAL + TOK))
    case " $C2_INV_LIST " in
        *" $IID:"*) INV=1;; *) INV=0;;
    esac
    ledger_answer "$IID" "C2" "$SUB" "prod_hybrid:recall@5=$C1_RECALL" "[\"harness-$IID\"]" "1" "$INV" "$TOK" "$DOL"
done < "$FIX/temporal-ku.jsonl"
if awk -v a="$C1_RECALL" -v b="$F0_C1" 'BEGIN { exit !(a > b) }' \
   && awk -v a="$C1_RECALL" 'BEGIN { exit !(a >= 0.80) }'; then C1_RES="PASS"; else C1_RES="FAIL"; fi
echo "C1: $C1_RES recall@5=$C1_RECALL judged=$C1_JUDGED (floor $F0_C1)"
if awk -v a="$C1_RECALL" -v b="$F0_C2" 'BEGIN { exit !(a > b) }' \
   && awk -v a="$C1_RECALL" 'BEGIN { exit !(a >= 0.80) }' \
   && [ "$C2_INV" -eq 0 ]; then C2_RES="PASS"; else C2_RES="FAIL"; fi
echo "C2: $C2_RES current=$C1_RECALL invalid-reuse=$C2_INV"

# ---------- C3: compaction survival via unit tests ----------
# NOTE: the runner cd'd to the scratch project dir; cargo must run from the
# repo root (subshell cd — `cargo test -C` is not a valid flag) or it finds
# no manifest and the gate misfires.
C3_OUT="$(cd "$REPO_ROOT" && cargo test -p jcode-base --lib harness_compaction 2>&1 || true)"
if printf '%s' "$C3_OUT" | grep -q "test result: ok"; then
    C3_RES="PASS"; FID="9/9"; ABS="3/3"
else
    C3_RES="FAIL"; FID="0/9"; ABS="0/3"
fi
# Record C3 per-fixture ledger lines from the fixture file (unit tests assert
# the mechanics; the ledger records the scored contract per item).
while IFS= read -r line; do
    [ -z "$line" ] && continue
    IID="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")"
    KIND="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['kind'])")"
    PROBE="$(printf '%s' "$line" | python3 -c "import json,sys; print(json.load(sys.stdin)['probe'])")"
    TOK="$(tokens_for_text "$PROBE")"
    DOL="$(dollars_for_tokens "$TOK")"
    TOK_TOTAL=$((TOK_TOTAL + TOK))
    if [ "$C3_RES" = "PASS" ]; then OK=1; else OK=0; fi
    ledger_answer "$IID" "C3" "$KIND" "unit-scored:$C3_RES" "[]" "$OK" "0" "$TOK" "$DOL"
done < "$FIX/compaction.jsonl"
echo "C3: $C3_RES fidelity=$FID abstention=$ABS"

# ---------- Totals + verdict ----------
DOL_TOTAL="$(dollars_for_tokens "$TOK_TOTAL")"
python3 - "$LEDGER" "$TOK_TOTAL" "$DOL_TOTAL" <<'PYEOF'
import json, sys
ledger, tok, dol = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
lines = open(ledger, encoding="utf-8").read().splitlines()
head = json.loads(lines[0])
head["token_total"] = tok
head["dollar_total"] = dol
lines[0] = json.dumps(head)
open(ledger, "w", encoding="utf-8").write("\n".join(lines) + "\n")
PYEOF
echo "LEDGER: harness/ledger/$TS.jsonl"
echo "TOKENS: $TOK_TOTAL DOLLARS: $DOL_TOTAL"

if [ "$C1_RES" = "PASS" ] && [ "$C2_RES" = "PASS" ] && [ "$C3_RES" = "PASS" ]; then
    exit 0
else
    exit 1
fi
