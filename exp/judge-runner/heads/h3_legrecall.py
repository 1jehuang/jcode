#!/usr/bin/env python3
"""h3_legrecall.py -- d3-P5 decomposition-aware eval head.

Retrieval-side (no judge cost for the recall half): leg-recall@k reported
PER LEG alongside micro recall. Judge-cost halves reuse the blind core:
  * entity-swap distractor ablation: the score MUST drop when a same-type
    entity is swapped into the gold span (true join sensitivity; a flat
    score means the judge grades topicality, not the join).
  * DiRe withheld-leg probe: PRECONDITION for any assembly claim (W3
    ruling), graded through run.sh --head=h1 with probe_type=leg_withheld,
    not an alongside metric.

Usage: h3_legrecall.py legs --retrieved RET.json --legs LEGS.json [--k 5]
       h3_legrecall.py swap --base LEDGER.json --swap LEDGER.json
  RET.json: {qid: [ranked ids]}; LEGS.json: {qid: {leg: [gold ids]}}.
"""

import argparse
import json
import sys


def legs(args):
    ret = json.load(open(args.retrieved))
    legspec = json.load(open(args.legs))
    per_leg_hits = {}
    micro_hit = micro_n = 0
    for qid, legmap in legspec.items():
        ranked = ret.get(qid, [])[:args.k]
        for leg, golds in legmap.items():
            golds = list(golds)
            if not golds:
                continue
            hit = 1 if any(g in ranked for g in golds) else 0
            h, n = per_leg_hits.get(leg, (0, 0))
            per_leg_hits[leg] = (h + hit, n + 1)
            micro_hit += hit
            micro_n += 1
    print(json.dumps({
        "k": args.k,
        "per_leg_recall": {leg: {"recall": h / n, "hits": h, "n": n}
                           for leg, (h, n) in sorted(per_leg_hits.items())},
        "micro_recall": (micro_hit / micro_n if micro_n else None),
        "micro_hits": micro_hit, "micro_n": micro_n}, indent=1))


def verdict_ids(row):
    return set((((row.get("judge") or {}).get("verdict") or {})
                .get("relevant_ids")) or [])


def swap(args):
    """Entity-swap ablation: join base and swap ledgers by (qid, arm);
    report the score drop. Flat score = judge grades topicality (FAIL)."""
    def load(path):
        rows = {}
        n_nw = 0
        for line in open(path):
            line = line.strip()
            if not line:
                continue
            r = json.loads(line)
            if r.get("record") != "judge":
                continue
            if r.get("error") is not None:
                n_nw += 1
                continue
            rows[(r["qid"], r.get("arm", "base"))] = verdict_ids(r)
        return rows, n_nw
    base, base_nw = load(args.base)
    swp, swp_nw = load(args.swap)
    same = drop = diff_n = 0
    for key, b in sorted(base.items()):
        if key not in swp:
            continue
        diff_n += 1
        if swp[key] == b:
            same += 1
        else:
            drop += 1
    print(json.dumps({
        "n_compared": diff_n,
        "n_unchanged": same,
        "n_changed": drop,
        "swap_stable_rate": (same / diff_n if diff_n else None),
        "sensitivity_note": "score MUST drop on entity-swap (same-type "
                            "entity swapped into gold span); flat = topicality "
                            "grading, not join sensitivity",
        "needswork": {"base": base_nw, "swap": swp_nw}}, indent=1))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("legs")
    p.add_argument("--retrieved", required=True)
    p.add_argument("--legs", required=True)
    p.add_argument("--k", type=int, default=5)
    s = sub.add_parser("swap")
    s.add_argument("--base", required=True)
    s.add_argument("--swap", required=True)
    a = ap.parse_args()
    if a.cmd == "swap":
        if a.base == a.swap:
            sys.exit("--base and --swap must be different ledger paths")
        swap(a)
    else:
        legs(a)


if __name__ == "__main__":
    main()
