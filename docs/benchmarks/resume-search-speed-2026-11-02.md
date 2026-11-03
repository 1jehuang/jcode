# Resume search speed baseline, November 2, 2026

## Status

The coordinator collected the verified baseline below before wiring the optimization.
The benchmark author deliberately did not run Rust builds concurrently with the coordinator.
The verified final candidate comparison is recorded below. The coordinator subsequently confirmed that all 269 picker tests and strict fuzzy-crate Clippy passed, with run IDs recorded below.

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
- **transcript_fuzzy_absent_repeated_words:** a real JSONL file contains a single 128 KiB decoded text segment of the same repeated words. An untimed literal absent scan first warms decoded text without preseeding fuzzy scores. Timed calls to `paths_match_scored` must return `None` for the absent fuzzy query. In the baseline this isolates repeated-word fuzzy work after decoded-text warming rather than JSON parsing or cold filesystem reads. The final candidate also includes a query-evidence cache, so later timed samples can reuse fuzzy evidence. The original emitted cache label is retained in the raw JSON but no longer describes every candidate sample. See the attribution caveat below.
- **ui_final_keystroke:** real `handle_search_key` calls at 100 and 1,000 sessions, each with 16 KiB metadata. Prefix `quart` setup and rebuilding are outside timing. The timed `z` key produces `quartz`. The picker must have no visible matches and no started receiver, ensuring the keystroke path does not launch fuzzy work. This is one final keystroke, not a typing burst or rendering benchmark.
- **suite_total:** includes fixtures, all timed loops, assertions, untimed cache warming, and picker setup. It excludes build and test-launch overhead.

## Correctness checks

The regular fixture test checks that the typo scores positively, the absent query scores zero, and a three-session worker returns the expected complete hit set or no hits. The ignored benchmark additionally asserts exact metadata sizes, nonliteral rank evidence, correct score polarity for every session and sample, complete worker membership, first-result presence or absence, warm transcript absence, UI query state, no UI matches and no fuzzy worker started from key handling. Timings are only emitted after their associated assertions succeed.

These are performance measurements, not runtime-daemon acceptance tests. They invoke the newly built test binary directly and do not depend on which long-lived Jcode daemon is active. They do not exercise terminal drawing, preview loading, remote SSH, real user histories, cold storage or transcript I/O in the worker matrix.

## Recording a valid comparison

Record revision and dirty state (`git rev-parse HEAD` and `git status --short`), the justrust run ID and `justrust show <id>` execution location, compiler/profile, CPU, sample count and the complete JSON lines for both runs. Run the baseline before implementation changes, then rerun the identical command after optimization. Compare the same case, query, size and cache-state rows. Keep baseline output even if a correctness assertion fails, but do not label failed runs as performance results. Do not infer a speedup from unrelated prior benchmarks or invent missing timings.

## Verified baseline

- Run ID: `20261102-191342864-2920165`, executed on hosted justrust, default test profile.
- Evidence: coordinator's complete task log `/tmp/jcode-bg-tasks/62284564am.output`, exit code 0, one benchmark passed and no failed tests.
- Toolchain: `nightly-2026-10-08`. The log does not identify the hosted CPU, so the local laptop CPU must not be substituted.
- Source context: benchmark commit `6b60aa721` plus coordinator's module-wiring edit in `transcript_search.rs`. The coordinator confirmed the proposed optimization was not wired for this baseline. An exact source snapshot and full dirty-state listing were not embedded in the task log.
- Two samples per case. Harness suite time: **2090.825399 ms**. Hosted overall time was 11.9 s, including compilation and synchronization, not the search suite's latency.
- Representative p50 observations at 100 x 64 KiB: typo metadata 200.984 ms, absent metadata 169.196 ms, typo worker completion 209.610 ms, absent worker completion 229.394 ms, observed typo first result 11.764 ms. Warm decoded transcript fuzzy absence was 3.421 ms.

These are a short baseline, not robust tail estimates. With two samples, the emitted p50 is the smaller sample and p95 is the larger sample. All baseline assertions passed. Full JSON emitted by the benchmark follows, with only the logging prefix removed:

```jsonl
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[50.676723,50.812923000000005],"p50_ms":50.676723,"p95_ms":50.812923000000005,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":[3.0848970000000002,3.319368],"p50_ms":3.0848970000000002,"p95_ms":3.319368,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[53.090036,53.145916],"p50_ms":53.090036,"p95_ms":53.145916,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[42.970709,43.627993],"p50_ms":42.970709,"p95_ms":43.627993,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":null,"query":"quartz","reason":"asserted_no_matches","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[58.140985,58.157815],"p50_ms":58.140985,"p95_ms":58.157815,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[200.98371,201.26249299999998],"p50_ms":200.98371,"p95_ms":201.26249299999998,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":[11.763625999999999,12.51721],"p50_ms":11.763625999999999,"p95_ms":12.51721,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[209.60981900000002,211.59017],"p50_ms":209.60981900000002,"p95_ms":211.59017,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[169.196483,170.981904],"p50_ms":169.196483,"p95_ms":170.981904,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":null,"query":"quartz","reason":"asserted_no_matches","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[229.394199,229.790602],"p50_ms":229.394199,"p95_ms":229.790602,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":131072,"cache":"decoded_text_warm_fuzzy_query_not_preseeded","case":"transcript_fuzzy_absent_repeated_words","debounce_included":false,"ms":[3.420839,3.5077],"p50_ms":3.420839,"p95_ms":3.5077,"query":"quartz","samples":2,"sessions":1}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_worker_started","case":"ui_final_keystroke","debounce_included":false,"ms":[0.242272,0.253581],"p50_ms":0.242272,"p95_ms":0.253581,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_worker_started","case":"ui_final_keystroke","debounce_included":false,"ms":[4.407795,5.149339],"p50_ms":4.407795,"p95_ms":5.149339,"query":"quartz","samples":2,"sessions":1000}
{"assertions":"passed","case":"suite_total","ms":2090.825399,"samples":2}
```

## Verified final candidate comparison

- Benchmark run: `20261102-192422604-2989590`, hosted justrust, same default test profile, same two samples per case and `nightly-2026-10-08`. Evidence: `/tmp/jcode-bg-tasks/254405xn67.output`, exit code 0, one benchmark passed. Search suite time was **347.492251 ms**, while total hosted time was 7.4 s including compilation and synchronization.
- The same task log records **17 scorer tests passed** in run `20261102-192414531-2986532`. The coordinator subsequently confirmed **all 269 picker tests passed** in hosted run `20261102-192504444-2994712`, and **strict fuzzy-crate Clippy passed** in run `20261102-192506201-2994711`. These test and lint results do not by themselves establish a live TUI acceptance workflow or a successful runtime reload.
- Candidate source context supplied by the coordinator: bounded query-local word-score reuse, with the cache's hot loop moved from the opt-level-0 TUI into the existing opt-level-3 `jcode-fuzzy` crate. No build-profile settings were changed. Concurrent token-evidence cache and warmup implementation changes were present in the same tree. The task log does not embed the complete source snapshot, dirty state or hosted CPU identity.

The following are observed p50 ratios from these short runs, not robust population estimates or a guarantee for real user histories. An absent query has no first result in either run. Worker measurements exclude debounce in both runs.

| Case | Fixture / query | Baseline ms | Candidate ms | Baseline / candidate |
| --- | --- | ---: | ---: | ---: |
| Metadata fuzzy rank | 100 x 64 KiB, `conifg` | 200.984 | 18.385 | 10.93x |
| Metadata fuzzy rank | 100 x 64 KiB, `quartz` | 169.196 | 16.843 | 10.05x |
| Worker first result | 100 x 64 KiB, `conifg` | 11.764 | 9.573 | 1.23x |
| Worker completion | 100 x 64 KiB, `conifg` | 209.610 | 28.784 | 7.28x |
| Worker completion | 100 x 64 KiB, `quartz` | 229.394 | 52.697 | 4.35x |
| UI final keystroke | 100 x 16 KiB, `quartz` | 0.242 | 0.285 | 0.85x |
| UI final keystroke | 1,000 x 16 KiB, `quartz` | 4.408 | 4.341 | 1.02x |
| Whole benchmark suite | All cases, fixtures and warming | 2090.825 | 347.492 | 6.02x |

UI timings are roughly unchanged at this sample count, with a small absolute regression at 100 sessions. The suite ratio combines all implementation changes and fixture/setup effects, not only word-score reuse.

### Transcript cache attribution

Baseline warm decoded transcript samples were `[3.420839, 3.5077]` ms. The candidate's sorted samples were `[0.00266, 0.361882]` ms. The coordinator reports chronological candidate execution as **0.361882 ms first**, then **0.00266 ms** after query-evidence cache reuse. The JSON arrays are sorted, so order cannot be recovered from those arrays alone. The second candidate scan is not an unseeded fuzzy query. Do not describe the 0.00266 ms p50 comparison as a word-score-only speedup, and do not treat both candidate transcript samples as having the same fuzzy-cache state. The candidate's first scan and evidence-reuse scan represent different workloads. A future benchmark can explicitly separate them rather than aggregating them into one distribution.

### Interim regression and correction

An earlier candidate, benchmark run `20261102-192207121-2969014` in `/tmp/jcode-bg-tasks/114439xz9u.output`, passed its assertions but was a performance regression overall: 2654.096908 ms suite time, and absent worker completion at 100 x 64 KiB was 478.087235 ms versus the 229.394199 ms baseline. Typo metadata improved only modestly to 180.492267 ms. The coordinator identified duplicate metadata work in the concurrent resident/cold fallback path and costly cache-loop execution in the opt-level-0 TUI. The final candidate above followed duplicate-pass fixes and moving the hot loop to the already optimized fuzzy crate. The earlier run was rejected as a performance result to ship, not hidden or relabeled as a successful optimization. Its correctness assertions did not fail.

### Complete candidate JSON

Original records follow with only the logging prefix removed. The transcript cache-label limitation above applies to these unchanged records.

```jsonl
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[4.7676680000000005,4.872439],"p50_ms":4.7676680000000005,"p95_ms":4.872439,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":[2.509725,2.724747],"p50_ms":2.509725,"p95_ms":2.724747,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[7.403915,7.633586],"p50_ms":7.403915,"p95_ms":7.633586,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[4.301156000000001,4.3072859999999995],"p50_ms":4.301156000000001,"p95_ms":4.3072859999999995,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":null,"query":"quartz","reason":"asserted_no_matches","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[13.397461,13.450571],"p50_ms":13.397461,"p95_ms":13.450571,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[18.385111,18.501082],"p50_ms":18.385111,"p95_ms":18.501082,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":[9.572697999999999,9.692859],"p50_ms":9.572697999999999,"p95_ms":9.692859,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[28.784494000000002,28.823294999999998],"p50_ms":28.784494000000002,"p95_ms":28.823294999999998,"query":"conifg","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_no_cross_query_score_warmup","case":"metadata_fuzzy_rank","debounce_included":false,"ms":[16.843042,16.853342],"p50_ms":16.843042,"p95_ms":16.853342,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_first_result","debounce_included":false,"ms":null,"query":"quartz","reason":"asserted_no_matches","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":65536,"cache":"resident_metadata_missing_transcript","case":"worker_completion","debounce_included":false,"ms":[52.697169,53.033321],"p50_ms":52.697169,"p95_ms":53.033321,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":131072,"cache":"decoded_text_warm_fuzzy_query_not_preseeded","case":"transcript_fuzzy_absent_repeated_words","debounce_included":false,"ms":[0.00266,0.361882],"p50_ms":0.00266,"p95_ms":0.361882,"query":"quartz","samples":2,"sessions":1}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_worker_started","case":"ui_final_keystroke","debounce_included":false,"ms":[0.28468200000000005,0.312882],"p50_ms":0.28468200000000005,"p95_ms":0.312882,"query":"quartz","samples":2,"sessions":100}
{"assertions":"passed","bytes_per_session":16384,"cache":"resident_metadata_no_worker_started","case":"ui_final_keystroke","debounce_included":false,"ms":[4.3407860000000005,4.727228],"p50_ms":4.3407860000000005,"p95_ms":4.727228,"query":"quartz","samples":2,"sessions":1000}
{"assertions":"passed","case":"suite_total","ms":347.49225099999995,"samples":2}
```
