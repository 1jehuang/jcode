# Reusable resume transcript search cache, November 2, 2026

## Scope

Resume transcript searches now share a process-wide, query-independent cache of
lowercased decoded JSON string values. Changing a query, switching between literal
and fuzzy scans, or closing and reopening the in-app resume overlay can reuse the
same text without rereading, reparsing, and lowercasing unchanged files.
The cache is not persisted to disk and does not survive restarting the TUI.
JSON keys are not searchable text. String boundaries and encounter order are
preserved so matching and fuzzy scoring retain their existing semantics.

Cache reads validate file metadata on the background search worker. Changed or
deleted files invalidate their entries. Unix fingerprints additionally include
device, inode, and change timestamps to detect same-length rewrites and atomic
replacement. Reads that race a file change are not published to the cache.
No cache lock spans file IO, parsing, or matching. Cancellation still applies.

Retention is limited to 256 files and 32 MiB of accounted decoded text with
least-recently-used eviction. Individual source files above 2 MiB or decoded
text above 4 MiB use the existing cancellable streaming scanner. These are
retained-cache budgets, not a cap on all process memory or concurrent readers.
The existing 150 ms search debounce is unchanged, so scan improvements must not
be interpreted as equivalent end-to-end input-to-results improvements.

## Reproduce

```bash
justrust test -p jcode-tui --lib session_picker
justrust test -p jcode-tui --lib benchmark_reusable_transcript_search -- --ignored --nocapture
```

The manual benchmark uses a temporary approximately 730 KiB JSONL transcript with
1,000 records. Absent literal terms require full scans. Its first scan is followed
by 30 distinct queries against the same file, testing reusable data rather than
memoization of one query. It excludes debounce, discovery, picker rendering,
and physical terminal paint. The filesystem page cache is not deliberately
cleared. One run is exploratory, not a reliable tail-latency estimate.

## Baseline

Before adding the cache, default hosted test profile, revision `e2beec537` plus
only the new benchmark, run `20261102-184053792-2751954`:

- First scan: 0.932 ms.
- Repeated different queries: p50 0.750 ms, p95 0.767 ms, 30 samples.

The hosted compile rebuilt the 271k-line TUI test crate in 8.74 seconds to run
one benchmark. Crate splitting is separate architecture work, not part of this
resume-search optimization.

## Cached results and validation

Same fixture and hosted default test profile, run
`20261102-184616687-2771104`, after caching and optimized newline scanning:

| Scan | Before | After |
| --- | ---: | ---: |
| First scan | 0.932 ms | 1.667 ms |
| Repeated-query p50 | 0.750 ms | 0.356 ms |
| Repeated-query p95 | 0.767 ms | 0.368 ms |

Repeated different queries took approximately 53% less scan time in this
fixture. The first scan pays the cost of retaining normalized strings and fully
decoding a cacheable file, even if its first record would match. That is a real
cold-path tradeoff, not an end-to-end speedup claim. Larger uncached files still
use the existing streaming early-exit path. The first implementation's per-byte
newline splitting cost 3.748 ms on hydration; using std's optimized newline
scanner reduced that to the 1.667 ms above.

All 259 picker regressions passed (16 manual tests ignored), hosted run
`20261102-184625828-2771103`. Coverage includes value-only parsing, malformed
records and snapshot streams, reuse across search instances, changed/deleted
files, same-length same-mtime inode replacement on Unix, cancellation, eviction,
and oversized-file/decoded-text fallback. Hosted `check -p jcode-tui --tests`
also passed. This is source-level and worker-interface validation, not a claim
that a particular installed TUI or shared daemon has already been updated.

Strict Clippy was attempted but is not green: the dependency-inclusive run
stopped on an existing redundant closure in `jcode-base/src/gateway.rs`, and
the `--no-deps` run reported existing unused items, collapsible conditionals,
and test-style findings elsewhere in the TUI. Neither reported a new cache
implementation diagnostic. Unrelated lint cleanup was deliberately excluded.
