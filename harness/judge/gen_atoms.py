#!/usr/bin/env python3
"""gen_atoms.py — derive Tier-2 judge items from dev fixtures (NO blind touch).

Atom derivation per 02 REPORT §Tier-2 (fixture-side, zero schema change):
- C1 recall: presence atoms = [gold_fact] (whole short-span fact, one atom);
  forbidden = [] (recall fixtures carry no forbidden field).
- C2 UPDATE/NEUTRAL: presence = [expected]; absence = forbidden[].
- C2 DELETE: presence = []; abstention_required=true; absence = forbidden[].
- C3 survival/decoy/latent: one atom per expected_contains element, ALL required;
  decoy/latent kinds ALSO carry the rumor as forbidden (rumor-quote penalty:
  decoy string stated as fact = FAIL). P-012 latent: forbidden=[decoy].
- C3 abstention (P-007..P-009): presence=[]; abstention_required=true;
  absence = [decoy] (the decoy is the plausible-but-unprotected rumor).

Gold candidates (step 1):
- recall: gold_fact verbatim.
- KU UPDATE/NEUTRAL: expected verbatim.
- KU DELETE: "No memories found matching '<topic>'" where topic = question
  stripped of leading "What is my/Where is my/What is..." — marker from the
  fixed set (memory_tests.rs:1108) + zero forbidden.
- C3 survival/decoy/latent: protected verbatim.
- C3 abstention: "No memories found matching '<probe noun>'".

Adversarial candidates (step 3), generated deterministically here:
- vague: question-echoing, value-free template per category (the Penfield
  threat — topical, no atom value, no forbidden string, no marker).
- forbidden: gold candidate + first forbidden string appended as fact
  (skipped where fixture has no forbidden strings).
- markerless: for DELETE/abstention items, a confident fabrication naming
  the forbidden value WITHOUT any marker (must FAIL: no marker + veto).

Output: atoms.jsonl — one record per item:
{id, question, atoms[], forbidden[], abstention_required, gold_answer,
 vague_answer, forbidden_answer|null, markerless_answer|null, spans}
spans = gold span context (recall: gold_turn id; KU: last event text;
C3: protected) — context only, not graded.
"""
import json
import re
import sys

FIX = "harness/fixtures"

MARKERS = ["no record", "forgotten", "unknown", "No memories found"]


def topic_of(question):
    q = question.rstrip("?")
    q = re.sub(r"^(What is|What are|Where is|When is|How many|How long|How often|How fast|Which|Who)\s+(my|the|does|do|did|are|is)\s+", "", q, flags=re.I)
    q = re.sub(r"^(my|the)\s+", "", q, flags=re.I)
    return q.strip() or question.rstrip("?")


def vague_for(question, kind):
    t = topic_of(question)
    return (
        f"The {t} was covered in the notes — it follows the usual arrangement "
        f"discussed for this topic. Check the record for the current detail."
    )


def main():
    items = []
    # C1 recall
    for line in open(f"{FIX}/recall.jsonl"):
        d = json.loads(line)
        gold = d["gold_fact"]
        items.append({
            "id": d["id"], "question": d["question"],
            "atoms": [gold], "forbidden": [],
            "abstention_required": False,
            "gold_answer": gold,
            "vague_answer": vague_for(d["question"], "recall"),
            "forbidden_answer": None,
            "markerless_answer": None,
            "spans": f"{d.get('gold_turn','')}: {gold}",
        })
    # C2 KU
    for line in open(f"{FIX}/temporal-ku.jsonl"):
        d = json.loads(line)
        st = d["subtype"]
        forb = d.get("forbidden", [])
        if st == "DELETE":
            t = topic_of(d["question"])
            gold = f"No memories found matching '{t}'"
            markerless = (
                f"Your {t} is {forb[0]}." if forb else None
            )
            items.append({
                "id": d["id"], "question": d["question"],
                "atoms": [], "forbidden": forb,
                "abstention_required": True,
                "gold_answer": gold,
                "vague_answer": vague_for(d["question"], "delete"),
                "forbidden_answer": (f"{gold} (previously {forb[0]})" if forb else None),
                "markerless_answer": markerless,
                "spans": d["events"][-1]["text"] if d.get("events") else "",
            })
        else:
            exp = d["expected"]
            items.append({
                "id": d["id"], "question": d["question"],
                "atoms": [exp], "forbidden": forb,
                "abstention_required": False,
                "gold_answer": exp,
                "vague_answer": vague_for(d["question"], "ku"),
                "forbidden_answer": (f"{exp} (formerly {forb[0]})" if forb else None),
                "markerless_answer": None,
                "spans": d["events"][-1]["text"] if d.get("events") else "",
            })
    # C3 compaction
    for line in open(f"{FIX}/compaction.jsonl"):
        d = json.loads(line)
        kind = d.get("kind")
        if kind == "abstention":
            t = topic_of(d["probe"])
            gold = f"No memories found matching '{t}'"
            items.append({
                "id": d["id"], "question": d["probe"],
                "atoms": [], "forbidden": [d["decoy"]] if d.get("decoy") else [],
                "abstention_required": True,
                "gold_answer": gold,
                "vague_answer": vague_for(d["probe"], "abstention"),
                "forbidden_answer": None,
                "markerless_answer": d.get("decoy"),
                "spans": "",
            })
        else:
            atoms = list(d.get("expected_contains", []))
            forb = [d["decoy"]] if kind in ("decoy", "latent-constraint") and d.get("decoy") else []
            gold = d.get("protected", "")
            items.append({
                "id": d["id"], "question": d["probe"],
                "atoms": atoms, "forbidden": forb,
                "abstention_required": False,
                "gold_answer": gold,
                "vague_answer": vague_for(d["probe"], "compaction"),
                "forbidden_answer": (f"{gold} Note: {d['decoy']}" if forb else None),
                "markerless_answer": None,
                "spans": gold,
            })
    out = sys.argv[1] if len(sys.argv) > 1 else "harness/judge/atoms.jsonl"
    with open(out, "w") as f:
        for it in items:
            f.write(json.dumps(it) + "\n")
    print(f"wrote {len(items)} items to {out}")
    # sanity: no blind ids
    blind_prefixes = ("H-", "M-", "B-")
    bad = [it["id"] for it in items if it["id"].startswith(blind_prefixes)]
    assert not bad, f"blind leak: {bad}"
    n_vague = sum(1 for it in items if it["vague_answer"])
    n_forb = sum(1 for it in items if it["forbidden_answer"])
    n_mark = sum(1 for it in items if it["markerless_answer"])
    print(f"vague={n_vague} forbidden-bearing={n_forb} markerless={n_mark}")


if __name__ == "__main__":
    main()
