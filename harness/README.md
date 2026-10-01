# Acceptance Harness (Phase 1 gate)

`./harness/run.sh [--bin <jcode>]` — see `../work/jcode/phase1/02-harness-spec.md`
(§1 layout, §4 scoring, §5.3 console contract) for the full contract.

Status (2026-09-26): GREEN as dev-gated regression tripwire.
PIN PASS / C1 0.935 vs floor 0.85 / C2 0.935 vs floor 0.909 invalid-reuse 0 /
C3 PASS / exit 0. Ledger: `ledger/2026-09-26-184635.jsonl`.

Margin caveat (strategy-review 2026-09-26): the 47 fixtures are DEV-GATED —
6 questions were paraphrased until F0 < memory, so margins are upper bounds,
not unbiased estimates. Do not cite externally without the locked held-out
re-baseline (auditor fix #1). K-005/R-005 are known-hard (fail both sides),
kept in the denominator by design. Numbers are "recall@5 (exact-match,
31 judged queries)", never "QA accuracy".
