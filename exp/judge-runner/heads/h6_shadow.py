#!/usr/bin/env python3
"""h6_shadow.py -- shadow counters + phase-gate summary (d2-P4).

Aggregates JUDGE_SHADOW_AGREE / DISAGREE / ERROR / NEEDSWORK outcomes from
one or more runner ledgers and prints the phase summary, mirroring the
prefilter shadow precedent already in the codebase:

  report   -- offline batch only (no gate impact)
  shadow   -- async judge cost, no user latency
  sampled  -- inline on a sample; latency bounded by the P5 shared deadline
  full     -- inline on every call (needs sampled-gate evidence first)

Phase is ADVISORY output (a string + the counts behind it), never a gate
decision: promotion stays coordinator-owned via the d2 P0 checklist + P4
ramp. Thresholds are CLI flags, not constants.

Usage: h6_shadow.py --ledger LEDGER.jsonl [--ledger MORE.jsonl ...]
         [--em EM.json] [--agree-bar 0.9] [--max-needswork-rate 0.05]
  With --em ({qid: bool}), agree/disagree split by EM-vs-judge; without,
  every clean row counts as agree (judge-only volume accounting).
"""

import argparse
import json


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ledger", action="append", required=True)
    ap.add_argument("--em", default=None)
    ap.add_argument("--agree-bar", type=float, default=0.9)
    ap.add_argument("--max-needswork-rate", type=float, default=0.05)
    a = ap.parse_args()
    em = json.load(open(a.em)) if a.em else {}
    agree = disagree = err = nw = 0
    for path in a.ledger:
        for line in open(path):
            line = line.strip()
            if not line:
                continue
            r = json.loads(line)
            if r.get("record") != "judge":
                continue
            if r.get("error") is not None:
                nw += 1
                continue
            if r.get("judge", {}).get("error") is not None:
                err += 1  # verdict-level error that still wrote a row
                continue
            qid = r["qid"]
            if qid in em:
                j_rel = bool((r["judge"]["verdict"] or {}).get("relevant_ids"))
                if bool(em[qid]) == j_rel:
                    agree += 1
                else:
                    disagree += 1
            else:
                agree += 1
    total = agree + disagree + err + nw
    agree_rate = agree / (agree + disagree) if (agree + disagree) else None
    nw_rate = nw / total if total else None
    if total == 0:
        phase = "report"
        why = "no rows"
    elif nw_rate is not None and nw_rate > a.max_needswork_rate:
        phase = "report"
        why = "needswork-rate above max (transport not yet clean)"
    elif agree_rate is not None and agree_rate >= a.agree_bar:
        phase = "sampled"
        why = "agree-bar met on ledger volume; sampled gate may proceed"
    else:
        phase = "shadow"
        why = "still accumulating async evidence"
    print(json.dumps({
        "JUDGE_SHADOW_AGREE": agree, "JUDGE_SHADOW_DISAGREE": disagree,
        "JUDGE_SHADOW_ERROR": err, "JUDGE_SHADOW_NEEDSWORK": nw,
        "total": total, "agree_rate": agree_rate, "needswork_rate": nw_rate,
        "phase": phase, "phase_why": why,
        "phase_note": "advisory only; promotion via d2 P0 checklist + P4 ramp",
        "full_gate_note": "full inline needs sampled-gate evidence first"},
        indent=1))


if __name__ == "__main__":
    main()
