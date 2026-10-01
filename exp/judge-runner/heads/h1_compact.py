#!/usr/bin/env python3
"""h1_compact.py -- d1-P2 compaction-probe head: item builder + 6-dim analysis.

Probes (4 + W1 extras): recall / artifact / continuation / decision, PLUS
the mandatory W1 multi-chain arm (2+ compactions) and an error-persistence
probe. Reports per-dimension pass deltas per arm pair; arm labels are
unblinded HERE in analysis only (run.sh grades blind).

Item shape (emitted by build_items, graded via run.sh --head=h1):
  {qid, probe_type, probe_question, answer, arm, step}
  arm examples: full | compacted | multi_chain | error_persist.

Analysis input: runner ledger path (h1 rows: judge.verdict.dims).
Analysis output: per-dimension pass rates per arm + compacted-minus-full
  and multi_chain-minus-full deltas (with 95% Wilson CI).

Usage:
  h1_compact.py build --sessions ...   # emit probe items (dev fixtures only)
  h1_compact.py analyze --ledger PATH  # per-dim deltas from a graded ledger
"""

import argparse
import json
import math
import sys
from collections import defaultdict

DIMS = ["accuracy", "context_awareness", "artifact_trail",
        "completeness", "continuity", "instruction_following"]

PROBE_TYPES = ["recall", "artifact", "continuation", "decision",
               "multi_chain", "error_persist"]


def wilson(p, n, z=1.96):
    """95% Wilson score interval half-width (z=1.96)."""
    if n == 0:
        return 1.0
    denom = 1 + z * z / n
    center = (p + z * z / (2 * n)) / denom
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return half


def build_items(pairs):
    """pairs: iterable of (arm, probe_type, probe_question, answer, tag).
    tag joins into qid so base/swap-style pairs stay matchable."""
    items = []
    for i, (arm, ptype, question, answer, tag) in enumerate(pairs):
        if ptype not in PROBE_TYPES:
            sys.exit("unknown probe_type %r (want one of %s)" % (ptype, PROBE_TYPES))
        items.append({"qid": "h1-%s-%04d" % (tag, i), "probe_type": ptype,
                      "probe_question": question, "answer": answer,
                      "arm": arm, "step": "h1"})
    return items


def analyze(ledger_path):
    """Per-dim pass rates per arm + deltas vs the `full` arm."""
    arms = defaultdict(lambda: defaultdict(lambda: [0, 0]))  # arm->dim->[pass,n]
    n_nw = 0
    for line in open(ledger_path):
        line = line.strip()
        if not line:
            continue
        r = json.loads(line)
        if r.get("record") != "judge" or r.get("head") != "h1":
            continue
        if r.get("error") is not None:
            n_nw += 1  # NEEDS-WORK counted separately, never graded
            continue
        v = (r.get("judge") or {}).get("verdict") or {}
        dims = v.get("dims") or {}
        for d in DIMS:
            if dims.get(d) in (0, 1):
                arms[r.get("arm", "?")][d][0] += dims[d]
                arms[r.get("arm", "?")][d][1] += 1
    if "full" not in arms:
        sys.exit("no `full` arm rows: cannot compute deltas")
    out = {"arms": {}, "deltas_vs_full": {}, "needswork": n_nw}
    for arm, dd in sorted(arms.items()):
        out["arms"][arm] = {d: {"pass": p, "n": n, "rate": (p / n if n else None)}
                            for d, (p, n) in sorted(dd.items())}
    for arm in sorted(arms):
        if arm == "full":
            continue
        out["deltas_vs_full"][arm] = {}
        for d in DIMS:
            pf, nf = arms["full"][d]
            pa, na = arms[arm][d]
            if nf and na:
                rf, ra = pf / nf, pa / na
                out["deltas_vs_full"][arm][d] = {
                    "delta": ra - rf, "ci95_half": wilson(ra, na) + wilson(rf, nf)}
            else:
                out["deltas_vs_full"][arm][d] = {"delta": None, "ci95_half": None}
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["build", "analyze"])
    ap.add_argument("--ledger", default=None)
    ap.add_argument("--pairs", default=None,
                    help="JSONL of [arm, probe_type, question, answer, tag] rows")
    a = ap.parse_args()
    if a.cmd == "analyze":
        if not a.ledger:
            sys.exit("--ledger required for analyze")
        print(json.dumps(analyze(a.ledger), indent=1))
    else:
        if not a.pairs:
            sys.exit("--pairs required for build")
        pairs = [json.loads(l) for l in open(a.pairs) if l.strip()]
        for it in build_items(pairs):
            print(json.dumps(it))


if __name__ == "__main__":
    main()
