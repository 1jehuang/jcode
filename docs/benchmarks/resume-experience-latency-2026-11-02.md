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

The runner uses hosted `justrust`, the `selfdev` profile, serial ignored tests, and retains unfiltered raw logs plus `metadata.json` and `results.json` under `target/resume-latency/`. It records revision, dirty state, profile, sample request, and measurement boundaries. Cargo `--config env.JCODE_RESUME_BENCH_SAMPLES` forwards sample configuration to hosted test execution rather than relying on arbitrary remote environment forwarding. Optional PTY measurements retain their separate raw logs. A failed command or absent benchmark rows fails the suite instead of reporting empty success.

## Still missing, deliberately

- Real SSH latency, connection reuse, remote failures/timeouts, and slow workspace/Git commands under controlled conditions.
- Cold OS page-cache/storage-device experiments and contention with other workloads.
- Actual input-device/terminal queue arrival through backend flush and physical compositor paint as one trace.
- Real sustained key-repeat cadence, background worker cancellation/resource use, and input backpressure under many competing jobs.
- Enter-to-attached-session usability and first response after resume. Opening/canceling the picker is not attach latency.
- Session-store scale with realistic mixtures of local/external/remote histories and long-lived memory pressure.

These are distinct future experiments, not claims covered by an offscreen render benchmark.
