#!/bin/bash
# baseline/grep.sh — F0 folder+grep baseline (harness spec Floor F0, §2 + §4).
# The naive alternative: plain term-overlap ranking over raw session
# transcripts. For each C1/C2 question, every corpus file scores by
# question-term hits; the item scores 1 iff the gold file ranks top-5 AND
# the returned evidence satisfies the identical scorer (§4.1/§4.2):
# gold fact/expected present verbatim, distractor-only never satisfies,
# UPDATE/DELETE forbidden values count as invalid-reuse.
# Corpus files hold FULL session text (all turns, superseded values,
# decoys, filler) — the pile a user would grep without a memory system.
#
# Usage: grep.sh <corpus_dir> <out_json>

set -euo pipefail

CORPUS_DIR="$1"
OUT_JSON="$2"
FIX_DIR="$(cd "$(dirname "$0")/../fixtures" && pwd)"

python3 - "$CORPUS_DIR" "$OUT_JSON" "$FIX_DIR" <<'PYEOF'
import json, os, re, sys

corpus, out, fixdir = sys.argv[1], sys.argv[2], sys.argv[3]

STOP = {"the", "a", "an", "is", "are", "was", "were", "what", "when",
        "where", "who", "how", "does", "do", "did", "my", "your", "it",
        "its", "in", "on", "at", "for", "of", "to", "and", "or", "i"}

def toks(s):
    return [t for t in re.findall(r"[a-z0-9]+", s.lower()) if t not in STOP]

files = {}
for fn in sorted(os.listdir(corpus)):
    if not fn.endswith(".txt"):
        continue
    files[fn[:-4]] = open(os.path.join(corpus, fn), encoding="utf-8").read()

def rank5(question):
    qt = toks(question)
    scored = []
    for stem, text in files.items():
        low = text.lower()
        hits = sum(1 for t in qt if t in low)
        scored.append((hits, stem))
    scored.sort(key=lambda x: (-x[0], x[1]))
    return [s for h, s in scored if h > 0][:5]

def verbatim(hay, needle):
    return needle.strip() != "" and needle.strip() in hay

# C1: gold file top-5 AND gold_fact verbatim in its text AND the matching
# span is the gold turn (distractor-only never satisfies).
c1_hits = c1_n = 0
with open(os.path.join(fixdir, "recall.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        c1_n += 1
        top = rank5(item["question"])
        if item["id"] not in top:
            continue
        text = files[item["id"]]
        if not verbatim(text, item["gold_fact"]):
            continue
        # Span check: gold turn text present (not just a distractor echo).
        gold_text = next(t["text"] for s in item["sessions"]
                         for t in s["turns"] if t["turn_id"] == item["gold_turn"])
        if verbatim(text, gold_text):
            c1_hits += 1

# C2: current-recall (expected verbatim, gold file top-5) + invalid-reuse
# (any forbidden verbatim in the top-ranked file evidence).
c2_cur = c2_cur_n = c2_inv = c2_inv_n = 0
with open(os.path.join(fixdir, "temporal-ku.jsonl"), encoding="utf-8") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        item = json.loads(line)
        st = item["subtype"]
        top = rank5(item["question"])
        ev = " ".join(files[s] for s in top if s in files)
        if st in ("UPDATE", "NEUTRAL"):
            c2_cur_n += 1
            if item["id"] in top and verbatim(ev, item["expected"]):
                c2_cur += 1
        if st in ("UPDATE", "DELETE"):
            c2_inv_n += 1
            if any(fb and verbatim(ev, fb) for fb in item.get("forbidden", [])):
                c2_inv += 1

with open(out, "w", encoding="utf-8") as f:
    json.dump({
        "c1_recall": (c1_hits / c1_n) if c1_n else 0.0,
        "c1_hits": c1_hits, "c1_n": c1_n,
        "c2_current": (c2_cur / c2_cur_n) if c2_cur_n else 0.0,
        "c2_cur": c2_cur, "c2_cur_n": c2_cur_n,
        "c2_invalid": (c2_inv / c2_inv_n) if c2_inv_n else 0.0,
        "c2_inv": c2_inv, "c2_inv_n": c2_inv_n,
    }, f, indent=1)
print(json.dumps({"c1_recall": round(c1_hits / c1_n, 4) if c1_n else 0,
                  "c2_current": round(c2_cur / c2_cur_n, 4) if c2_cur_n else 0,
                  "c2_invalid": round(c2_inv / c2_inv_n, 4) if c2_inv_n else 0}))
PYEOF
