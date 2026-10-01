#!/usr/bin/env python3
"""h2_agree.py -- d2-P1 agreement-ledger head: EM-vs-judge agreement/CWR/abstain.

Reads a listwise runner ledger (+ an EM key: qid -> bool) and reports:
  agreement + Cohen kappa WITH 95% CI per category (C1/C2/C3 x K/P split),
  abstain precision/recall (recall needs --abst-gold, else null), and CWR
  (conditional wrongness rate: P(EM wrong | judge says relevant-set
  non-empty)). The CWR numeric bar comes from SHADOW volume, never n=47 --
  this head reports the rate; the bar is set by the coordinator from
  shadow counts.

NEEDS-WORK rows are counted separately and excluded from every rate
(fail-closed: never graded fail-open).

Usage: h2_agree.py --ledger LEDGER.jsonl --em EM.json [--category CAT.json]
         [--abst-gold ABST.json]
  EM.json: {qid: bool}; CAT.json (optional): {qid: "C1-K"|...};
  ABST.json (optional): {qid: bool} gold "should-abstain" key.
"""

import argparse
import json
import math
import sys
from collections import defaultdict


def kappa_and_ci(a, b, c, d):
    """2x2 table [[a=both-yes, b=em-yes-judge-no],
    [c=em-no-judge-yes, d=both-no]] -> (kappa, ci95_half)."""
    n = a + b + c + d
    if n == 0:
        return None, None
    po = (a + d) / n
    pe = ((a + b) * (a + c) + (c + d) * (b + d)) / (n * n)
    if pe >= 1.0:
        return None, None
    k = (po - pe) / (1 - pe)
    se = math.sqrt(max(0.0, po * (1 - po)) / (n * (1 - pe) ** 2))
    return k, 1.96 * se


def bump(t, e_ok, j_ok):
    if e_ok and j_ok:
        t[0] += 1
    elif e_ok:
        t[1] += 1
    elif j_ok:
        t[2] += 1
    else:
        t[3] += 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ledger", required=True)
    ap.add_argument("--em", required=True, help="JSON {qid: bool}")
    ap.add_argument("--category", default=None, help="JSON {qid: category}")
    ap.add_argument("--abst-gold", default=None,
                    help="JSON {qid: bool} gold should-abstain key (else recall=null)")
    a = ap.parse_args()
    em = json.load(open(a.em))
    cat = json.load(open(a.category)) if a.category else {}
    abst_gold = json.load(open(a.abst_gold)) if a.abst_gold else {}
    tables = defaultdict(lambda: [0, 0, 0, 0])  # "ALL" + per-category
    abst_tp = abst_fp = abst_fn = abst_n = 0
    cwr_num = cwr_den = 0
    n_nw = n_missing_em = 0
    for line in open(a.ledger):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        if r.get("record") != "judge" or r.get("head") != "listwise":
            continue
        if r.get("error") is not None:
            n_nw += 1
            continue
        qid = r["qid"]
        if qid not in em:
            n_missing_em += 1
            continue
        j_rel = bool(((r.get("judge") or {}).get("verdict") or {}).get("relevant_ids"))
        e_ok = bool(em[qid])
        bump(tables["ALL"], e_ok, j_rel)
        if qid in cat:
            bump(tables[cat[qid]], e_ok, j_rel)
        if r.get("abstained") is True:
            abst_n += 1
            if qid in abst_gold:
                if abst_gold[qid]:
                    abst_tp += 1
                else:
                    abst_fp += 1
        elif abst_gold.get(qid) is True:
            abst_fn += 1
        if j_rel:
            cwr_den += 1
            if not e_ok:
                cwr_num += 1
    out = {"per_category": {}, "abstain": {}, "cwr": {}, "needswork": n_nw,
           "missing_em": n_missing_em}
    for c, (x, y, z, w) in sorted(tables.items()):
        n = x + y + z + w
        if n == 0:
            continue
        k, ci = kappa_and_ci(x, y, z, w)
        out["per_category"][c] = {"n": n, "agree": (x + w) / n,
                                  "kappa": k, "kappa_ci95_half": ci,
                                  "table": {"em_yes_judge_yes": x,
                                            "em_yes_judge_no": y,
                                            "em_no_judge_yes": z,
                                            "em_no_judge_no": w}}
    denom_p = abst_tp + abst_fp
    denom_r = abst_tp + abst_fn
    out["abstain"] = {
        "n_abstained": abst_n,
        "precision": (abst_tp / denom_p if denom_p else None),
        "recall": (abst_tp / denom_r if denom_r else None),
        "recall_note": None if a.abst_gold
        else "no --abst-gold key: recall is null by design, not by data",
        "tp": abst_tp, "fp": abst_fp, "fn": abst_fn}
    out["cwr"] = {"rate": (cwr_num / cwr_den if cwr_den else None),
                  "n_judge_relevant": cwr_den,
                  "note": "bar from SHADOW volume, never n=47"}
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
