# [05] Stabilize `benchmark_resume_loading_reports_timings`

## What it does

Test-only change. `benchmark_resume_loading_reports_timings` (upstream test)
now invalidates the shared session-list cache immediately before loading and
retries once on a short count, so its `sessions.len() >= 100` assertion
observes a settled directory listing instead of a poisoned cache entry.

## Why

The test writes 120 session files into a `JCODE_HOME` tempdir, then calls
`load_sessions()`. Tests that call `load_sessions()` *without* taking
`lock_test_env` resolve the same process-global `JCODE_HOME` and can finish a
scan mid-write, caching a partial listing (~60 entries) under this test's
cache key `(sessions_dir, scan_limit, external_sessions)`. The benchmark's own
load then hits that entry within the 5s TTL and sees fewer than 100 sessions —
the observed parallel-suite flake.

`lock_test_env` cannot prevent this: it serializes env mutation between
lock-holders, but lock-less readers still see the holder's env and share the
global cache.

## Why not fix the cache itself

The root cause is an upstream correctness gap: `SessionListCacheEntry` has no
staleness guard beyond the 5s TTL, so a scan of a still-being-written
directory can be served to later readers (in production: `/resume` may miss a
session created moments earlier). A proper fix — e.g. keying the entry by the
directory mtime — is a ~14-line change to `loading.rs`, but it is an
**upstream bug fix**, not something this patch series carries. Noted here so
it can be PR'd separately; the test-side mitigation below keeps our runs
deterministic in the meantime.

## Semantics

- `invalidate_session_list_cache()` clears any entry cached mid-write; the
  retry covers the residual interleave where a concurrent scan completes in
  the tiny window between invalidation and this test's own cache check.
- No production code changed; `load_sessions` semantics are untouched.

## Affected files

- `crates/jcode-tui/src/tui/session_picker/loading_tests.rs` —
  invalidate + retry in `benchmark_resume_loading_reports_timings`.

## Upstream candidacy

The test-side mitigation is not worth upstreaming on its own, but the
underlying cache-staleness gap (missing dir-mtime/generation key) is a good
small upstream PR candidate — flagged here per fork rules rather than carried
as a local patch.
