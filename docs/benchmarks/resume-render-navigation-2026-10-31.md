# Resume picker render/navigation performance, 2026-10-31

## Goal and method

Reduce loaded-session revisit latency and idle/scroll frame work without retaining an unbounded set of prepared transcripts. All measurements below used hosted `justrust test`, the same default test profile, synthetic sessions, and Ratatui's `TestBackend`. No application binary was built for these measurements. Times are medians in microseconds, not terminal presentation latency.

Commands:

```bash
justrust test -p jcode-tui --lib benchmark_resume_op -- --ignored --nocapture
justrust test -p jcode-tui --lib benchmark_resume_stable_frames -- --ignored --nocapture
justrust test -p jcode-tui --lib performance_tests -- --include-ignored --nocapture
```

Existing operation baseline: `20261031-040039561-611884`.
Stable viewport baseline: `20261031-040559581-640382`.
Final stable frames and scoped regressions: `20261031-041514223-694110`.
Final operation measurements: `20261031-041523744-694856`.

## Results

| Operation | Before (us) | After (us) |
| --- | ---: | ---: |
| Stable 80-turn preview idle draw | 336 | 189 |
| Stable 80-turn loaded-session revisit | 48,812 | 186 |
| Navigate + full render, 50 sessions | 4,736 | 408 |
| Navigate + full render, 500 sessions | 8,503 | 438 |
| Navigate + full render, 2,000 sessions | 19,987 | 550 |
| Session list only, 50 sessions | 400 | 132 |
| Session list only, 200 sessions | 1,326 | 141 |
| Session list only, 1,000 sessions | 6,295 | 198 |
| Session list only, 3,000 sessions | 19,159 | 343 |
| Existing scroll benchmark, 80 turns, preview only | 66,274 | 248 |
| Existing scroll benchmark, 200 turns, preview only | 211,314 | 259 |

The existing scroll benchmark alternates full-picker and preview-only geometries. Its baseline repeatedly rebuilt the single retained layout rather than representing a pure fixed-geometry idle redraw. The new stable viewport benchmark measures idle and loaded-session revisits separately, using one persistent terminal and one geometry.

## Attribution and changes

* The stable baseline independently measured a whole `SessionInfo` clone at 25 us and a complete transcript hash at 121 us. Borrow the selected session in short scopes rather than clone it each frame. Cache only the transcript digest, with mutable session access invalidating the entry and backing-list reseeds clearing all reference-indexed digests. Continue hashing dynamic header, config, load-state, deferred-render epoch and extras inputs every frame.
* Switching sessions discarded all prepared markdown/wrapping state. Retain prepared previews in an LRU bounded by both entry count and owned-buffer bytes. Revisit tests verify the original wrapped-line allocation is reused and rendered pixels are unchanged.
* Initial retention achieved 347 us revisits. A second attribution run (`20261031-041409509-688756`) measured 343 us revisits, 190 us idle and 152 us scanning cache allocations for byte accounting. Store each immutable cache's byte count once on construction and transfer it alongside the cache. Final revisits are 186 us, with no transcript-size accounting traversal on hits.
* Session list rendering formatted every session despite only a small viewport being visible. Resolve exact item heights cheaply, compute Ratatui-compatible whole-item bounds, and format only that range. Scrollbar totals and offsets still refer to the complete list. The inexpensive height pass remains O(session count), not O(rendered text).

## Bounds and correctness

Additional prepared-preview payload is limited to 8 entries and 16 MiB, excluding the currently displayed preview and small bounded container/allocator overhead. An oversized current preview still renders, but is not retained when leaving it. Transcript fingerprints retain at most 16 entries. Picker memory diagnostics now include active and retained prepared previews and fingerprint storage.

Four non-ignored scoped regression tests cover allocation/pixel reuse, mutable/load/reseed invalidation, exact byte accounting and LRU/oversize eviction, and virtualized-vs-full Ratatui buffer/state equivalence across 420 combinations of dimensions, offsets and selections, including variable prompt/crash row heights and a group header. All four plus the ignored stable-frame benchmark passed in the final hosted run.

Concurrent relevance-ranking work changed search keystroke costs, which are deliberately not claimed as gains from this patch. Its measured regression was reported to the ranking owner. Construction timings were essentially unchanged at 1,000 and 5,000 sessions. No unrelated loading, search-ranking, or rendering-fidelity optimizations were included here.
