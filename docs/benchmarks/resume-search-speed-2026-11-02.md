# Resume search speed baseline, November 2, 2026

## Status

The harness is ready for baseline collection. No timing results are recorded here yet.
The benchmark author deliberately did not run Rust builds concurrently with the coordinator.

## Wiring and commands

The coordinator should add this test-only child module to `transcript_search.rs`:

```rust
#[cfg(test)]
#[path = "search_speed_tests.rs"]
mod search_speed_tests;
```

Run from the existing checkout, using the same host, profile, source tree and sample count before and after optimization:

```bash
justrust test -p jcode-tui --lib search_speed_fixture_preserves_typo_and_absent_semantics -- --test-threads=1
justrust test -p jcode-tui --lib benchmark_resume_search_speed -- --ignored --nocapture --test-threads=1
```

The default is two samples per case, intended to keep baseline execution below 15 seconds, excluding compilation and hosted transfer. This is a target, not a measured claim. For a quick smoke run set `JCODE_SEARCH_SPEED_SAMPLES=1`. For a more stable follow-up set it to 5 or 10. The accepted range is clamped to 1 through 100. Confirm that the remote runner forwards this variable before relying on a nondefault count. Every output row records the actual sample count.

Do not run the full ignored suite by accident. Other ignored benchmarks have different fixtures and runtime budgets. Do not run simultaneous builds or timing workloads. Preserve the complete justrust output rather than piping away diagnostic or timing context.

## Cases and timing boundaries

Each output line begins with `SEARCH_SPEED ` and contains a JSON object. Durations are wall-clock milliseconds. Sorted individual samples, nearest-rank p50 and p95, sample count, cache state and fixture size are emitted. With only two samples, p95 is the maximum and should not be interpreted as a robust tail estimate.

- **metadata_fuzzy_rank:** 100 sessions, each exactly 16 KiB or 64 KiB of repeated `config deployment performance synchronization throughput ` metadata. Both `conifg` (typo hit) and `quartz` (absent) run through the real `fuzzy_metadata_rank` wrapper. The timed loop includes rank assertions and excludes fixture setup. Each wrapper invocation is independent, with no explicitly preseeded fuzzy-query cache.
- **worker_first_result / worker_completion:** the same 100-session matrices run through actual `TranscriptSearch::reset` and `poll`. Reset and metadata copying happen outside timing. Backdating `started_at` bypasses the 150 ms debounce. Timing starts just before the poll that spawns the worker and includes spawn, prioritization, fuzzy ranking, missing-path handling, bounded result merging and scheduler delay. Polling sleeps 100 microseconds, so first-result latency is observed latency rather than an instrumented worker timestamp. All transcript paths are deliberately absent under a private temporary directory. Typo queries must yield all 100 results. Absent queries must yield none, and their first-result value is explicitly `null`, not the completion time. Each worker must finish within 10 seconds.
- **transcript_fuzzy_absent_repeated_words:** a real JSONL file contains a single 128 KiB decoded text segment of the same repeated words. An untimed literal absent scan first warms decoded text without preseeding fuzzy scores. Timed calls to `paths_match_scored` must return `None` for the absent fuzzy query. This isolates repeated-word fuzzy work after decoded-text warming rather than JSON parsing or cold filesystem reads.
- **ui_final_keystroke:** real `handle_search_key` calls at 100 and 1,000 sessions, each with 16 KiB metadata. Prefix `quart` setup and rebuilding are outside timing. The timed `z` key produces `quartz`. The picker must have no visible matches and no started receiver, ensuring the keystroke path does not launch fuzzy work. This is one final keystroke, not a typing burst or rendering benchmark.
- **suite_total:** includes fixtures, all timed loops, assertions, untimed cache warming, and picker setup. It excludes build and test-launch overhead.

## Correctness checks

The regular fixture test checks that the typo scores positively, the absent query scores zero, and a three-session worker returns the expected complete hit set or no hits. The ignored benchmark additionally asserts exact metadata sizes, nonliteral rank evidence, correct score polarity for every session and sample, complete worker membership, first-result presence or absence, warm transcript absence, UI query state, no UI matches and no fuzzy worker started from key handling. Timings are only emitted after their associated assertions succeed.

These are performance measurements, not runtime-daemon acceptance tests. They invoke the newly built test binary directly and do not depend on which long-lived Jcode daemon is active. They do not exercise terminal drawing, preview loading, remote SSH, real user histories, cold storage or transcript I/O in the worker matrix.

## Recording a valid comparison

Record revision and dirty state (`git rev-parse HEAD` and `git status --short`), the justrust run ID and `justrust show <id>` execution location, compiler/profile, CPU, sample count and the complete JSON lines for both runs. Run the baseline before implementation changes, then rerun the identical command after optimization. Compare the same case, query, size and cache-state rows. Keep baseline output even if a correctness assertion fails, but do not label failed runs as performance results. Do not infer a speedup from unrelated prior benchmarks or invent missing timings.
