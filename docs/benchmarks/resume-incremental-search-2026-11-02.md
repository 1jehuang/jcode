# Incremental resume search, November 2, 2026

## Goal and baseline

Use time spent typing instead of repeatedly discarding all query-dependent work.
The primary metric is query edit to first matching background result, including
the actual production debounce, measured separately from input-handler cost and
full search completion. Matching/ranking must remain equivalent to a fresh scan.

The existing text cache makes scans cheaper but the worker previously waited
150 ms even when every transcript was resident. Before these changes, hosted
run `20261102-191334163-2919512`, revision `ab917dcdd` plus the benchmark, measured
five warm query edits/backspaces at 153.290, 153.134, 152.342, 152.553, and
152.314 ms to first result. This is a small exploratory sample, not a percentile
or physical terminal-paint measurement. Filesystem caches were not flushed.

## Incremental work

- A bounded query-independent preparation queue starts decoding small candidate
  transcripts while the user types. A superseded query does not cancel the
  current bounded preparation. There is one worker, at most eight queued paths,
  deduplication of pending paths, and at most 32 scheduling hints per edit.
  The text cache's existing source-size, decoded-size, and retained-memory
  limits still apply. Uncacheable large files are not read by this queue.
- Completed per-token literal and fuzzy evidence is stored separately, tied to
  the exact validated decoded-text generation. Keeping a completed word while
  typing another word, reordering terms, or backspacing can reuse the earlier
  outcome. Completed positive and negative evidence survives cancellation of
  later token work. Incomplete negatives are never published.
- Extending a literal token can reuse an absent shorter prefix as proof of
  absence. A positive prefix is not proof of the longer token, and literal
  negatives never exclude fuzzy candidates. Fuzzy first-match scores retain
  the existing segment and file ordering.
- Resident evidence is scheduled before the cold debounce. Only cheap,
  nonblocking cache hints run on the UI thread. File fingerprint validation,
  matching, and fuzzy scoring stay on a background search worker. Cache hints
  are not trusted as content evidence. Changes/deletions invalidate evidence.
- Remaining query-dependent cold scans still wait until 150 ms after the most
  recent edit. New queries cancel those obsolete scans. Fully resident searches
  can finish without that wait. This is not elimination of the cold-search
  debounce or a persistent full-text index across process restarts.

Per-token evidence is bounded to 4,096 entries and 1 MiB of accounted entry/key
storage, with a 4,096-byte token admission limit. Weak generation references do
not pin evicted transcript strings. Existing shared word-score scratch is
preserved through warm/cold scans rather than rebuilt for every token.

## Reproduce

```bash
justrust check --remote -p jcode-tui --tests
justrust test --remote -p jcode-tui --lib session_picker
justrust test --remote -p jcode-tui --lib benchmark_incremental_query_readiness -- --ignored --nocapture
```

The manual readiness benchmark prepares one approximately 165 KiB JSONL
transcript, then searches `searchal`, `searchalpha`, `searchalpha bet`,
`searchalpha beta`, and backspaces to `searchalpha`. It polls at a modeled
1 ms interval and stops each observation at the first matching worker result.
It does not time terminal rendering, the live application scheduler, physical
paint, full-corpus completion, or real SSH storage.

Deterministic tests move the cold deadline far into the future rather than
asserting a fragile sub-150 ms machine-speed threshold. They require warm
queries and preparations started during typing to serve the newest query before
any cold work can run. Fresh streaming comparisons cover literal/fuzzy pairs,
Unicode, corrupt JSONL records, cross-file matches, score ordering, prefix
extensions, backspacing, file invalidation, and cancellation.

## Initial integrated measurement

Hosted run `20261102-192423221-2989724`, same benchmark and profile, measured
2.383, 2.141, 2.135, 2.129, and 2.131 ms to first matching result. Compared
with the baseline's 152–153 ms, this shows the warm path no longer pays the
150 ms debounce in this fixture. It does not establish live frame latency,
tail percentiles, or equivalent savings for cold/large histories.

Initial integrated picker validation passed 277 tests (18 manual tests ignored),
run `20261102-192411114-2986252`. Input-handler benchmark observations stayed
at 1.580/1.930/1.420 ms for its three edits over 500 sessions. Additional
generation-map cache lookup refinements are validated separately below.

## Final bounded-cache validation

The generation-map cache uses borrowed-token hash lookups rather than scanning
all retained files on every hit. Cleanup happens on insertions, and batch LRU
eviction shrinks sparse per-generation tables. Prefix inference probes at most
64 UTF-8 boundaries and skips oversized queries, falling back to a normal scan
instead of doing unbounded prefix hashing while holding the evidence lock.

Hosted picker run `20261102-193201139-3045014` passed 281 tests, with 18 manual
tests ignored. This includes the full 4,096-evidence/256-generation cache case,
amortized cleanup, bounded long/Unicode query fallback, and the independent
streaming equivalence matrix. The three input-handler observations were
1.538/1.914/1.536 ms over 500 sessions.

Final same-fixture readiness run `20261102-193212533-3045976` measured 2.411,
2.146, 2.139, 2.139, and 2.137 ms. This preserves the approximately 2–3 ms warm
worker-result readiness observed before the lookup-bound refinement.
`check -p jcode-tui --tests` passed. Scoped non-strict Clippy passed as run
`20261102-193213201-3046039`, with existing project warnings. Its one cache
conditional-style warning was then corrected without changing matching logic.

The integrated implementation is in `4c0ef6357`, coordinated with the shared
word-score optimization. Runtime installation/reload is a separate step and
must respect concurrent source-fingerprint checks, not bypass them.
