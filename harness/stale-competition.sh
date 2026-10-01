#!/bin/bash
# stale-competition.sh — temporal-validity writer fitness arm (08-stale-writer).
#
# Drives the SHIPPED write path (`memory import`, `memory forget`) with NEW
# fixture ids (blind sets frozen — never H-/K- ids), then asserts per-query
# ranked retrieval through the REAL prod path (`stale_rank` bin wraps
# MemoryManager::find_similar_hybrid end to end).
#
# UPDATE leg: import OLD then NEW per fixture. Pre-writer both rows stay
#   active (stale competition: old outranks or co-ranks). Post-writer the R2
#   scan supersedes old (active=false, Supersedes edge new->old): assert new
#   ranks first + old id absent from ranked output.
# GUARD leg: H-006-shaped near-miss (mentorship Ana/Theo vs buddy Ana/Ravi).
#   Both must STAY active with no edge between them, before and after.
# DELETE leg: import state value, `memory forget` (tombstone default) then a
#   second value with --privacy (hard erase). Assert tombstone row
#   (present, active=false, superseded_by null) + forbidden absent from ranks;
#   privacy leg asserts the row is gone + forbidden absent.
#
# Usage: harness/stale-competition.sh <JCODE_BIN> <STALE_RANK_BIN>
# Exit 0 iff every UPDATE/GUARD/DELETE assertion passes.
set -u
JCODE_BIN="${1:?usage: stale-competition.sh <JCODE_BIN> <STALE_RANK_BIN>}"
RANK_BIN="${2:?usage: stale-competition.sh <JCODE_BIN> <STALE_RANK_BIN>}"
# Canonicalize before the cd below (relative paths would break).
JCODE_BIN="$(readlink -f "$JCODE_BIN")"
RANK_BIN="$(readlink -f "$RANK_BIN")"

SCRATCH="$(mktemp -d /home/sk/.jcode/scratch/stale-comp.XXXXXX)"
export JCODE_HOME="$SCRATCH/home"
export JCODE_PROJECT_DIR="$SCRATCH/proj"
mkdir -p "$JCODE_HOME" "$JCODE_PROJECT_DIR"
cd "$JCODE_PROJECT_DIR"
# Hermetic model cache: jcode_dir() honors JCODE_HOME, so without this every
# embed call in scratch would re-download the 90MB ONNX from HuggingFace
# (proven 2026-09-29: fresh-scratch arm died at S-006 with "error decoding
# response body" on bad network). Seed from the real user cache; the bins
# then never touch the network. Fails soft (warn) if real cache is absent —
# bins fall back to download as before.
REAL_MODEL_DIR="${HOME}/.jcode/models/all-MiniLM-L6-v2"
if [ -f "$REAL_MODEL_DIR/model.onnx" ] && [ -f "$REAL_MODEL_DIR/tokenizer.json" ]; then
    mkdir -p "$JCODE_HOME/models/all-MiniLM-L6-v2"
    cp "$REAL_MODEL_DIR/model.onnx" "$REAL_MODEL_DIR/tokenizer.json" "$JCODE_HOME/models/all-MiniLM-L6-v2/"
else
    echo "WARN: real model cache absent ($REAL_MODEL_DIR); scratch embeds may download" >&2
fi

# --- fixtures: NEW ids, paraphrased shapes (never blind H-/K- ids) ---
cat > "$SCRATCH/fixtures.json" << 'EOF'
{"leg":"UPDATE","id":"S-001","query":"When is standup time?","forbidden":"9am","old":"my standup is at 9am","new":"standup update: moved to half past ten, still fifteen minutes"}
{"leg":"UPDATE","id":"S-002","query":"Which evenings are my gym sessions?","forbidden":"Monday","old":"gym sessions on Monday evenings","new":"training update: gym sessions moved to Wednesday evenings"}
{"leg":"UPDATE","id":"S-003","query":"Which editor do I use?","forbidden":"vim","old":"favorite editor is vim","new":"editor switch: favorite editor is now helix"}
{"leg":"UPDATE","id":"S-004","query":"What is my coffee order?","forbidden":"flat white","old":"coffee order is a flat white","new":"caffeine update: coffee order changed to black filter"}
{"leg":"UPDATE","id":"S-005","query":"When is the project deadline?","forbidden":"March","old":"project deadline is March 1st","new":"planning update: project deadline extended to April 15th"}
{"leg":"UPDATE","id":"S-006","query":"Who is my thesis advisor?","forbidden":"Rao","old":"my thesis advisor is Dr. Rao","new":"advisor change: my thesis advisor is now Dr. Osei"}
{"leg":"GUARD","id":"S-G1","query":"Who is paired with Ana for mentorship?","forbidden":"","old":"mentorship pairs Ana with Theo, kickoff in September","new":"buddy program pairs Ana with Ravi for the summer"}
{"leg":"DELETE","id":"S-D1","query":"What is the garden shed code?","forbidden":"7712","old":"garden shed code is 7712","new":""}
{"leg":"DELETE","id":"S-D2","query":"What is the padlock combination?","forbidden":"12-34","old":"padlock combination is 12-34-56","new":""}
EOF

python3 - "$SCRATCH" << 'PYEOF'
import json, sys
scratch = sys.argv[1]
items = [json.loads(l) for l in open(scratch + '/fixtures.json') if l.strip()]
olds, news, updates, deletes, guards = [], [], [], [], []
import time
now = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())
def entry(mid, content):
    return {"id": mid, "category": "fact", "content": content,
            "tags": ["stale-arm"], "search_text": content.lower(),
            "created_at": now, "updated_at": now, "access_count": 0,
            "active": True, "strength": 1, "confidence": 1.0, "reinforcements": []}
for it in items:
    if it['leg'] == 'UPDATE':
        updates.append(it)
        olds.append(entry(it['id'] + '-old', it['old']))
        news.append(entry(it['id'] + '-new', it['new']))
    elif it['leg'] == 'GUARD':
        guards.append(it)
        olds.append(entry(it['id'] + '-a', it['old']))
        news.append(entry(it['id'] + '-b', it['new']))
    else:
        deletes.append(it)
        olds.append(entry(it['id'], it['old']))
json.dump(olds, open(scratch + '/olds.json', 'w'))
json.dump(news, open(scratch + '/news.json', 'w'))
json.dump({'updates': updates, 'guards': guards, 'deletes': deletes},
          open(scratch + '/legs.json', 'w'))
print(f"fixtures: {len(olds)} olds, {len(news)} news")
PYEOF

GRAPH_GLOB="$JCODE_HOME/memory/projects/*.json"
pass=0; fail=0
report() { # report <id> <ok:0/1> <detail>
    if [ "$2" = 0 ]; then pass=$((pass+1)); echo "PASS $1 $3"; else fail=$((fail+1)); echo "FAIL $1 $3"; fi
}
graph_file() { ls $GRAPH_GLOB 2>/dev/null | head -1; }

echo "--- UPDATE+GUARD leg: import OLD, then NEW (real remember path) ---"
"$JCODE_BIN" memory import "$SCRATCH/olds.json" --scope project > /dev/null
"$JCODE_BIN" memory import "$SCRATCH/news.json" --scope project > /dev/null
GF="$(graph_file)"; [ -n "$GF" ] || { echo "MALFORMED: no graph file"; exit 2; }
# Retrieval needs stored vectors (hybrid pools require embedding.is_some();
# import no longer embeds on the write path). Snapshot + embed a COPY for
# ranking; graph-state assertions above/below always read the live $GF.
EMB="$SCRATCH/embedded.json"
"$RANK_BIN" --embed_corpus="$GF" --out="$EMB" 2>/dev/null | python3 -c "import json,sys;print('embedded:', json.load(sys.stdin)['embedded'])"
# The `memory forget` CLI is writer-new: base binaries lack it. Probe once;
# DELETE legs SKIP (not fail) where the subcommand does not exist.
if "$JCODE_BIN" memory forget --help > /dev/null 2>&1; then HAS_FORGET=1; else HAS_FORGET=0; fi

for iid in S-001 S-002 S-003 S-004 S-005 S-006; do
    newid="${iid}-new"; oldid="${iid}-old"
    query="$(python3 -c "import json;print([i['query'] for i in map(json.loads,open('$SCRATCH/fixtures.json')) if i['id']=='$iid'][0])")"
    old_active="$(python3 -c "import json;print(json.load(open('$GF'))['memories']['$oldid']['active'])")"
    sup_by="$(python3 -c "import json;print(json.load(open('$GF'))['memories']['$oldid'].get('superseded_by'))")"
    rank_out="$("$RANK_BIN" --corpus="$EMB" --query="$query" --limit=10 2>"$SCRATCH/rank-$iid.stderr")"
    first="$(echo "$rank_out" | python3 -c "import json,sys;r=json.load(sys.stdin)['ranked'];print(r[0]['id'] if r else 'EMPTY')")"
    old_present="$(echo "$rank_out" | python3 -c "import json,sys;print('$oldid' in [x['id'] for x in json.load(sys.stdin)['ranked']])")"
    if [ "$old_active" = "False" ] && [ "$sup_by" = "$newid" ] && [ "$first" = "$newid" ] && [ "$old_present" = "False" ]; then
        report "$iid" 0 "old tombstoned superseded_by=new, new ranks first, old absent"
    else
        report "$iid" 1 "old_active=$old_active sup_by=$sup_by first=$first old_present=$old_present out=$rank_out"
    fi
done

# GUARD: both stay active, unlinked; mentorship (Theo) answer must rank first.
gquery="Who is paired with Ana for mentorship?"
g_rank="$("$RANK_BIN" --corpus="$EMB" --query="$gquery" --limit=10 2>"$SCRATCH/rank-guard.stderr")"
g_first="$(echo "$g_rank" | python3 -c "import json,sys;r=json.load(sys.stdin)['ranked'];print(r[0]['id'] if r else 'EMPTY')")"
g_a="$(python3 -c "import json;print(json.load(open('$GF'))['memories']['S-G1-a']['active'])")"
g_b="$(python3 -c "import json;print(json.load(open('$GF'))['memories']['S-G1-b']['active'])")"
g_link="$(python3 -c "
import json
g=json.load(open('$GF'))
edges=g.get('edges',{})
ids={'S-G1-a','S-G1-b'}
print(any(src in ids and any(x.get('target') in ids for x in el) for src, el in edges.items()))")"
if [ "$g_a" = "True" ] && [ "$g_b" = "True" ] && [ "$g_link" = "False" ] && [ "$g_first" = "S-G1-a" ]; then
    report "S-G1" 0 "both active unlinked, mentorship answer first"
else
    report "S-G1" 1 "a=$g_a b=$g_b linked=$g_link first=$g_first out=$g_rank"
fi

echo "--- DELETE leg: forget (tombstone) + forget --privacy (erase) ---"
if [ "$HAS_FORGET" = 0 ]; then
    echo "SKIP S-D1 no forget subcommand in this binary"
    echo "SKIP S-D2 no forget subcommand in this binary"
else
"$JCODE_BIN" memory forget S-D1 > "$SCRATCH/forget-d1.out" 2>&1
# Re-embed the post-forget snapshot: tombstone state must be what ranks see.
EMB_D1="$SCRATCH/embedded-d1.json"
"$RANK_BIN" --embed_corpus="$GF" --out="$EMB_D1" > /dev/null 2>&1
d1_state="$(python3 -c "
import json
m=json.load(open('$GF'))['memories'].get('S-D1')
print('MISSING' if m is None else f\"active={m['active']} sup={m.get('superseded_by')}\")")"
d1_rank="$("$RANK_BIN" --corpus="$EMB_D1" --query="What is the garden shed code?" --limit=10 2>/dev/null)"
d1_hit="$(echo "$d1_rank" | python3 -c "import json,sys;print('S-D1' in [x['id'] for x in json.load(sys.stdin)['ranked']])")"
if [ "$d1_state" = "active=False sup=None" ] && [ "$d1_hit" = "False" ]; then
    report "S-D1" 0 "tombstoned (row kept, inactive) + absent from ranks"
else
    report "S-D1" 1 "state=$d1_state ranked_hit=$d1_hit out=$d1_rank"
fi

"$JCODE_BIN" memory forget S-D2 --privacy > "$SCRATCH/forget-d2.out" 2>&1
EMB_D2="$SCRATCH/embedded-d2.json"
"$RANK_BIN" --embed_corpus="$GF" --out="$EMB_D2" > /dev/null 2>&1
d2_gone="$(python3 -c "import json;print('S-D2' not in json.load(open('$GF'))['memories'])")"
d2_rank="$("$RANK_BIN" --corpus="$EMB_D2" --query="What is the padlock combination?" --limit=10 2>/dev/null)"
d2_hit="$(echo "$d2_rank" | python3 -c "import json,sys;print('S-D2' in [x['id'] for x in json.load(sys.stdin)['ranked']])")"
if [ "$d2_gone" = "True" ] && [ "$d2_hit" = "False" ]; then
    report "S-D2" 0 "privacy-erased (row gone) + absent from ranks"
else
    report "S-D2" 1 "gone=$d2_gone ranked_hit=$d2_hit out=$d2_rank"
fi
fi

echo "stale-competition: pass=$pass fail=$fail scratch=$SCRATCH"
[ "$fail" = 0 ]
