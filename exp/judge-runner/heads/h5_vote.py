#!/usr/bin/env python3
"""h5_vote.py -- selective n=3 majority vote on the flagged slice (d2-P3).

Triggers (any one): EM != judge, swap-flip, parse-retry consumed,
backup-model verdict. Takes the flagged qids, fans out to 3 grading calls
at temp > 0 through the blind core (run.sh invoked 3x with per-call
--temperature), and records majority: ledger `votes` (the 3 verdict id-sets
as sorted lists) + `unanimous` (all 3 identical).

Cost stays ~1.1-1.3x base: +2 calls x flagged fraction (~10-30%).

Usage:
  h5_vote.py items --ledger LEDGER.jsonl --flagged QIDS.txt --out ITEMS.jsonl
      # emit re-grade items for the flagged slice (grade via run.sh 3x,
      # then vote)
  h5_vote.py vote --a A.jsonl --b B.jsonl --c C.jsonl --out VOTES.jsonl
      # majority over 3 graded ledgers joined by (qid, arm)
"""

import argparse
import json
import sys


def verdict_ids(row):
    return sorted((((row.get("judge") or {}).get("verdict") or {})
                   .get("relevant_ids")) or [])


def items(args):
    flagged = {l.strip() for l in open(args.flagged) if l.strip()}
    n_hit = n_miss = 0
    with open(args.out, "w") as f:
        for line in open(args.ledger):
            line = line.strip()
            if not line:
                continue
            r = json.loads(line)
            if r.get("record") != "judge" or r.get("error") is not None:
                continue  # NEEDS-WORK never enters a vote
            if r["qid"] in flagged:
                f.write(json.dumps({"qid": r["qid"], "arm": r.get("arm", "base"),
                                    "step": "h5-vote"}) + "\n")
                n_hit += 1
            else:
                n_miss += 1
    print("flagged items=%d unflagged=%d -> %s" % (n_hit, n_miss, args.out),
          file=sys.stderr)


def load_ids(path):
    rows = {}
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        if r.get("record") != "judge" or r.get("error") is not None:
            continue
        rows[(r["qid"], r.get("arm", "base"))] = verdict_ids(r)
    return rows


def vote(args):
    if len({args.a, args.b, args.c}) < 3:
        sys.exit("three DIFFERENT graded ledgers required")
    A, B, C = load_ids(args.a), load_ids(args.b), load_ids(args.c)
    keys = sorted(set(A) & set(B) & set(C))
    n_una = 0
    with open(args.out, "w") as f:
        for key in keys:
            va, vb, vc = A[key], B[key], C[key]
            votes = [va, vb, vc]
            # majority = the id-set appearing >= 2x; ties (3 distinct)
            # keep the temp-0-equivalent first vote and mark split.
            counts = {}
            for v in votes:
                counts.setdefault(tuple(v), 0)
                counts[tuple(v)] += 1
            maj = max(counts.items(), key=lambda kv: kv[1])
            una = va == vb == vc
            n_una += una
            f.write(json.dumps({"qid": key[0], "arm": key[1], "head": "h5",
                                "majority_ids": list(maj[0]),
                                "majority_count": maj[1],
                                "split": maj[1] < 2,
                                "votes": votes, "unanimous": una,
                                "error": None}) + "\n")
    print("voted=%d unanimous=%d -> %s" % (len(keys), n_una, args.out))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("items")
    p.add_argument("--ledger", required=True)
    p.add_argument("--flagged", required=True)
    p.add_argument("--out", required=True)
    v = sub.add_parser("vote")
    v.add_argument("--a", required=True)
    v.add_argument("--b", required=True)
    v.add_argument("--c", required=True)
    v.add_argument("--out", required=True)
    a = ap.parse_args()
    (vote if a.cmd == "vote" else items)(a)


if __name__ == "__main__":
    main()
