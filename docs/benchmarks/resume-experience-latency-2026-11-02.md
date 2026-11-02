# Resume experience latency coverage, November 2, 2026

## What was measured already

| Surface | Existing measurement | Important limitation |
| --- | --- | --- |
| Startup | `scripts/bench_resume_picker.py`: process launch to loading marker, populated list, and cancellation/terminal restoration using a real PTY | Fixed geometry, current on-disk store, medians. PTY bytes are not physical display paint. Never selects/resumes a session. |
| Session discovery | `loading_tests.rs` loading-phase benchmarks, including ignored real-store cases | Discovery is not a rendered usable preview. |
| Search input | `benchmark_resume_search_keystroke_latency` in `transcript_search_tests.rs` | Handler duration, not completion of background transcript search. |
| Warm rendering | `benchmark_resume_stable_frames` in `performance_tests.rs` | Prepared preview hits, idle/revisit and hashing costs, not first visits. |
| Cold transcript preparation | `viewport_preview_cold_benchmark` in `ui_prepare/viewport_preview_tests.rs` | Renderer microbenchmark, not disk loading, input scheduling, or terminal flush. |
| Live rendering | `client:draw-stats`, `client:render-stats` | Generic rendering/flush diagnostics, not selection-specific readiness stages. |

Historical results and methodology remain in `resume-render-navigation-2026-10-31.md` and `resume-search-input-2026-10-30.md`. Those medians should not be directly compared with a different build profile, fixture, geometry, or timing boundary.

## Added repeatable coverage

`session_picker/benchmark_matrix.rs` measures full picker draws through Ratatui's in-memory `TestBackend`:

- Opening/loading placeholder, separately from list discovery.
- List construction, key handler, and list frame at 100, 1,000, and 5,000 sessions. Fixture construction and backend allocation are outside the timed windows.
- First-visit resident transcript misses, warm revisits, idle frames, preview scrolling, width changes, and resizing back.
- Three geometries: 48x24, 120x40, 200x60. Three transcript sizes: 1, 20, and 150 user/assistant turns.
- Cache eviction beyond the eight retained prepared previews, distinguished from renderer-cache coldness.
- Ten navigation events with a frame per event versus ten coalesced events and one final frame. These measure throughput, not a realistic keyboard-repeat/scheduler model.
- Unicode/CJK/emoji/combining marks, large tool output, and edit diffs at narrow and normal widths.

Cold resident cases use distinct per-session/per-sample content and clear public renderer/highlight caches. They do not claim a cold filesystem, network, or every process-global cache. Resident fixtures suppress workspace/filesystem probing; disk-backed cases are separate.

`session_picker/benchmark_io.rs` uses temporary real Jcode snapshots and Codex JSONL files, actual discovery/loaders, and a 2,048-message fixture:

- Transcript **data** readiness, not rendered preview readiness.
- Metadata query typing and complete background transcript matching, including debounce and a middle-only needle outside the preview window.
- Debounced-query cancellation, not preemption of an already-running disk read.
- Wall time, modeled polling sleep, and active caller wall time. Active wall time is not CPU time. Polling uses a modeled 16ms interval, not the live application's scheduler.
- Failures/timeouts counted separately, never recorded as zero-latency successes. Filesystem page cache is warm, SSH is offline/unmeasured.

Distributions report p50/p95/p99/max. The resident matrix also counts observations above 16/50/100ms. Twenty observations are useful for exploratory medians, **not reliable p99 estimates**. Use at least 100 for tail exploration and repeat independent runs. Timing tests are ignored/manual rather than flaky CI thresholds. Normal tests cover fixture isolation, percentile calculation, and diagnostic correctness.

## Live readiness diagnostics

`client:resume-latency` exposes bounded, rolling measurements for the current picker lifetime:

- Handler entry to selection frame prepared.
- Handler entry to matching transcript frame prepared.
- Accepted asynchronous workspace-facts completion.
- Picker render duration, preview cache hits/misses, superseded selections, and selected transcript failures.

Initial selection starts at picker initialization, not physical key arrival. Preparation ends inside the picker, before backend flush or emulator/compositor paint. Workspace cache hits are not included in hydration timing. Successful-stage distributions must be interpreted alongside failure and supersession counts to avoid hiding bad experiences. Measurements retain at most 256 observations per stage and no session content in their output.

## Reproduce and retain results

```bash
python3 scripts/bench_resume_suite.py --samples 20
# More useful tail exploration, larger runtime:
python3 scripts/bench_resume_suite.py --samples 100
# Also measure real PTY startup and cancellation on a specific installed binary:
python3 scripts/bench_resume_suite.py --samples 20 --binary /absolute/path/to/jcode
python3 -m unittest scripts/test_bench_resume_suite.py
```

The runner uses hosted `justrust`, the `selfdev` profile (currently `opt-level = 0`, not an optimized distribution build), serial ignored tests, and retains unfiltered raw logs plus `metadata.json` and `results.json` under `target/resume-latency/`. It records revision, dirty state, profile, sample request, and measurement boundaries. Cargo `--config env.JCODE_RESUME_BENCH_SAMPLES` forwards sample configuration to hosted test execution rather than relying on arbitrary remote environment forwarding. Optional PTY measurements retain their separate raw logs. A failed command or absent benchmark rows fails the suite instead of reporting empty success.

## Baseline from the expanded suite

Recorded November 2, 2026 at 23:37 UTC, revision `b7c71bd15` with a dirty working tree, hosted Linux execution, `selfdev` (`opt-level = 0`), 20 observations per metric. This is an exploratory baseline, not an old/new optimization claim or reliable tail estimate. All 28 scenario rows completed and all 160 disk/search attempts succeeded with no timeouts. Raw structured output is at `target/resume-latency/2026-11-02-expanded-baseline-retry/results.json` in this checkout.

Resident markdown navigation at 120x40, milliseconds, p50:

| History | Cold handler to frame | Warm revisit | Width-change frame | Warm preview scroll |
| --- | ---: | ---: | ---: | ---: |
| 1 turn | 1.089 | 0.213 | 1.067 | 0.228 |
| 20 turns | 8.793 | 0.227 | 8.767 | 0.244 |
| 150 turns | 61.009 | 0.235 | 60.941 | 0.253 |

Other surfaces, milliseconds:

| Measurement | p50 | p95 |
| --- | ---: | ---: |
| Loading placeholder frame, 120x40 | 0.122 | 0.132 |
| List frame, 100 sessions | 0.339 | 0.355 |
| List frame, 1,000 sessions | 0.362 | 0.373 |
| List frame, 5,000 sessions | 0.468 | 0.476 |
| Picker construction, 5,000 sessions, fixture creation excluded | 7.483 | 7.648 |
| Evicted 20-turn preview rebuild, renderer caches may be warm | 6.453 | 6.468 |
| Ten navigation events, a frame each, total throughput | 63.465 | 63.633 |
| Ten navigation events, one final frame, total throughput | 6.333 | 6.352 |
| 800-line tool-output first frame, 120x40 | 8.287 | 8.490 |
| Jcode snapshot to transcript data ready, modeled polling | 16.117 | 16.133 |
| Codex JSONL to transcript data ready, modeled polling | 16.111 | 16.117 |
| Jcode complete transcript search, including debounce | 193.732 | 193.745 |
| Codex complete transcript search, including debounce | 176.876 | 176.893 |

Interpretation and next targets:

1. **Cold preparation and width invalidation still scale with transcript size.** A 150-turn cold preview is about 61ms at all three geometries, while cached revisits are below 0.5ms. This supports prioritizing viewport-bounded preparation, incremental work, or safely scheduled prefetch rather than further list-drawing micro-optimization. These are hypotheses to test, not changes delivered by this benchmark addition.
2. **Avoid preparing intermediate selections unnecessarily.** The burst experiment exposes roughly ten times the total work when every intermediate selection is rendered. It does not establish live key-repeat latency or prove the production scheduler currently coalesces those events.
3. **Search readiness is a different budget from input responsiveness.** The production debounce is 150ms. This fixture's 177–194ms search wall times include that debounce, worker work, and modeled 16ms polling. Active caller wall time was only about 0.11–0.90ms, which is not worker CPU time.
4. **Disk data readiness is quantized by this harness's poll model.** Approximately 16ms does not mean the storage read itself takes 16ms. Use live selection/transcript stage diagnostics to measure the actual application wait.
5. **Tail exploration needs more runs.** One narrow edit-diff cold frame reached 20.336ms despite a 5.654ms median. Keep max and threshold counts visible rather than relying only on smooth medians.

Hosted run IDs: matrix `20261102-153718864-2120330`, disk/search `20261102-153739520-2121950`. An earlier full `--tests` selfdev check passed as `20261102-153504539-2099845`; its following benchmark process was interrupted by an unrelated shared-daemon reload and was not used as the completed baseline.

Validation: 221 normal picker tests passed, including five live-stage regressions, and four Python runner tests passed. A separate five-sample hosted run completed all 28 rows with exactly five observations/attempts, verifying nondefault sample forwarding. An isolated real-PTY smoke on binary `b7c71bd15-dirty-c2ceed1e5065` verified selection/transcript/workspace stages, two superseded selections during three rapid inputs, and the 256-sample bound. Its tester was cleaned up without resuming a session. Live smoke timings are not treated as a statistical baseline.

## Still missing, deliberately

- Real SSH latency, connection reuse, remote failures/timeouts, and slow workspace/Git commands under controlled conditions.
- Cold OS page-cache/storage-device experiments and contention with other workloads.
- Actual input-device/terminal queue arrival through backend flush and physical compositor paint as one trace.
- Real sustained key-repeat cadence, background worker cancellation/resource use, and input backpressure under many competing jobs.
- Enter-to-attached-session usability and first response after resume. Opening/canceling the picker is not attach latency.
- Session-store scale with realistic mixtures of local/external/remote histories and long-lived memory pressure.

These are distinct future experiments, not claims covered by an offscreen render benchmark.
