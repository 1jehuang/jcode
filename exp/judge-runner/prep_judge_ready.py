#!/usr/bin/env python3
"""prep_judge_ready.py -- the missing prep join (host: python, not cmd_prep).

queries.jsonl (QueryRecord{qid,session,turn,query,origin_memory_ids}) +
pool.jsonl (PoolRecord{qid,candidates:[{id,content,retrievers[]}]})
-> judge_ready.jsonl (JudgeInput{qid,query,candidates}).

One JudgeInput per line, per src/bin/memory_recall_bench.rs JudgeInput
(:680-684). Query text joins from queries.jsonl by qid (:431-439);
candidates join from pool.jsonl by qid. Queries with no pool entry are
skipped (reported on stderr); pools with no query entry are an error
(they would grade blind against unknown context -- fail-closed).

Usage: prep_judge_ready.py --labels=DIR [--out=PATH]
  DIR defaults to $MEMORY_BENCH_DIR/labels (or ~/jcode-memory-bench/labels).
  Out defaults to DIR/judge_ready.jsonl.
"""

import argparse
import json
import os
import sys


def load_by_qid(path, key_desc):
    rows = {}
    with open(path) as f:
        for ln, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                d = json.loads(line)
            except json.JSONDecodeError as e:
                sys.exit("bad %s line %d in %s: %s" % (key_desc, ln, path, e))
            qid = d.get("qid")
            if not qid:
                sys.exit("missing qid at %s line %d" % (path, ln))
            if qid in rows:
                sys.exit("duplicate qid %s in %s" % (qid, path))
            rows[qid] = d
    return rows


def main():
    default_labels = os.path.join(
        os.environ.get("MEMORY_BENCH_DIR",
                       os.path.join(os.environ.get("HOME", ""), "jcode-memory-bench")),
        "labels",
    )
    ap = argparse.ArgumentParser()
    ap.add_argument("--labels", default=default_labels)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    qpath = os.path.join(a.labels, "queries.jsonl")
    ppath = os.path.join(a.labels, "pool.jsonl")
    out = a.out or os.path.join(a.labels, "judge_ready.jsonl")
    queries = load_by_qid(qpath, "query")
    pools = load_by_qid(ppath, "pool")

    orphans = sorted(set(pools) - set(queries))
    if orphans:
        sys.exit("fail-closed: %d pool qids with no query entry (e.g. %s); "
                 "refusing to grade blind against unknown context"
                 % (len(orphans), orphans[:3]))
    skipped = sorted(set(queries) - set(pools))
    n = 0
    with open(out, "w") as f:
        for qid in sorted(set(queries) & set(pools)):
            q, p = queries[qid], pools[qid]
            cands = p.get("candidates")
            if not isinstance(cands, list) or not cands:
                print("skip %s: empty candidate list" % qid, file=sys.stderr)
                skipped.append(qid)
                continue
            for i, c in enumerate(cands):
                if not isinstance(c, dict) or "id" not in c or "content" not in c:
                    sys.exit("bad candidate %d in pool %s" % (i, qid))
                c.setdefault("retrievers", [])
            f.write(json.dumps({"qid": qid, "query": q["query"],
                                "candidates": cands}) + "\n")
            n += 1
    print("wrote %d judge inputs -> %s (queries-without-pool skipped: %d)"
          % (n, out, len(skipped)), file=sys.stderr)
    if skipped:
        print("skipped qids: %s" % " ".join(sorted(skipped)[:10]), file=sys.stderr)


if __name__ == "__main__":
    main()
