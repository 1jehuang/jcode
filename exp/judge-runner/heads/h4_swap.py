#!/usr/bin/env python3
"""h4_swap.py -- swap-agreement head (d2-P2 + X2 L2 amendment).

Joins a base listwise ledger with its candidate-REVERSED re-grade by
(qid, arm) and reports the swap_agree rate: fraction of pairs whose
relevant_id SETS are identical. Flips (disagreements) route to H5
(selective n=3 vote). Covers the bench listwise arm (bench --swap emits
matched rows) AND the L2 retry acceptance bar (X2 amendment: an L2 retry
is accepted only if the swap re-grade agrees). Pointwise Tier-2 exempt
(no candidate order exists there).

Usage: h4_swap.py --base BASE.jsonl --swapped SWAPPED.jsonl [--out OUT.jsonl]
  With --out, writes per-qid {qid, arm, swap_agree, base_ids, swap_ids}
  rows for H5 triage; without, prints the summary only.
"""

import argparse
import json
import sys


def verdict_ids(row):
    return sorted((((row.get("judge") or {}).get("verdict") or {})
                   .get("relevant_ids")) or [])


def load(path):
    rows, n_nw = {}, 0
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        if r.get("record") != "judge":
            continue
        if r.get("error") is not None:
            n_nw += 1  # NEEDS-WORK never agrees: excluded from the rate
            continue
        rows[(r["qid"], r.get("arm", "base"))] = r
    return rows, n_nw


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--swapped", required=True)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    if a.base == a.swapped:
        sys.exit("--base and --swapped must be different ledger paths")
    base, base_nw = load(a.base)
    swp, swp_nw = load(a.swapped)
    agree = disagree = missing = 0
    flips = []
    out_f = open(a.out, "w") if a.out else None
    for key in sorted(base):
        if key not in swp:
            missing += 1
            continue
        b, s = verdict_ids(base[key]), verdict_ids(swp[key])
        row = {"qid": key[0], "arm": key[1], "swap_agree": b == s,
               "base_ids": b, "swap_ids": s}
        if b == s:
            agree += 1
        else:
            disagree += 1
            flips.append(key[0])
        if out_f:
            out_f.write(json.dumps(row) + "\n")
    if out_f:
        out_f.close()
    n = agree + disagree
    print(json.dumps({
        "n_compared": n, "n_agree": agree, "n_flip": disagree,
        "swap_agree": (agree / n if n else None),
        "missing_in_swapped": missing,
        "flip_qids": flips,
        "h5_route": "flip qids route to h5_vote.py (selective n=3)",
        "needswork": {"base": base_nw, "swapped": swp_nw}}, indent=1))


if __name__ == "__main__":
    main()
