# Allocator discrepancy controls, 2026-11-18

## Question and scope

Earlier first-pass profiler runs showed stable sampled application live heap but higher `stats.allocated` and private memory. The comparison also had different thread populations. These controls rerun the same private 16-session native Responses fixture against the same two pinned first-pass profiler binaries. They do **not** measure all later optimizations or establish a lower production RAM floor.

Fixture: eight 128 KiB tool outputs per session, asserted exact output/call identity and ordering, local synthetic SSE responses, isolated `JCODE_HOME`, runtime directory and socket, `JCODE_TEST_SESSION=1`. No real inference or user session history. Both binaries used `prof:true,prof_active:true,lg_prof_sample:15,prof_accum:true`. Thread counts and names were captured at each sample.

Diagnostic script and full JSON artifacts are under `~/.jcode/scratch/ram-fork/allocator-attribution-probe.py` and the directories below. These depend on earlier local fixture artifacts and are not a portable standalone benchmark.

## Measurements

Bytes below are allocator counters, not sampled heap estimates. Default worker settings were retained except in the explicitly labeled control. Runs were sequential, with reversed binary order in the second pair.

| Configuration / order | Before active allocated | After active allocated | Before completed allocated | After completed allocated | Active threads before/after | Completed threads before/after |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| Default, before then after | 73,988,624 | 75,993,048 | 37,521,360 | 42,034,376 | 23 / 23 | 23 / 34 |
| Default, after then before | 75,322,256 | 74,895,256 | 39,660,696 | 38,596,008 | 23 / 23 | 23 / 23 |
| `tcache:false`, before then after | 68,530,544 | 68,211,640 | 28,280,352 | 28,076,744 | 23 / 23 | 23 / 23 |
| `tcache:false`, four Tokio workers, after then before | 68,122,352 | 68,083,208 | 27,757,952 | 27,780,904 | 11 / 11 | 11 / 11 |

Artifact labels: `attr-default-{before,after}-20261118`, `attr-reverse-{before,after}-20261118`, `attr-notcache-{before,after}-20261118`, `attr-fixed-notcache-{before,after}-20261118`. Each contains `results.json`, heap profiles and fixture diagnostics.

Default active private memory before/after was 142.906/145.070 MiB in the first pair, then 151.684/146.066 MiB in reversed order. The sign is not stable. Cache-disabled active private memory was 137.523/140.559 MiB; with four workers it was 133.668/134.711 MiB. These resident measurements remain influenced by profiler, page and allocator state and do not establish a production improvement.

## Interpretation and limits

The earlier positive allocator difference is **not reproducible as a stable live-payload increase**: reversing order changes its sign, and disabling thread caches reduces the difference to -318,904 bytes active / -203,608 completed, or -39,144 active / +22,952 completed with the controlled worker count. This strongly implicates cache-mediated accounting/retention and runtime thread population in the earlier discrepancy. It does not prove the exact origin of every byte, or equivalence on all workloads.

The official jemalloc manual notes that thread caching can leave some statistics incompletely merged, and distinguishes application allocated, active pages, allocator metadata and resident memory. Sampled profiling is also not an exact census. Therefore `allocator_live_bytes` must not be interpreted as exact retained application payload or compared without matching runtime/configuration controls.

**No production cache, arena, worker-count, purge or decay policy is changed.** Disabling caches or reducing concurrency could exchange CPU/latency for RAM and fails the user's pure-efficiency requirement. These are attribution controls only. Continue searching for avoidable work and copies, with separate exact-behavior and CPU/allocation acceptance tests.

Reference: official jemalloc manual, sections on statistics and `stats.allocated`/`stats.active`/`stats.resident` (jemalloc.net, retrieved 2026-11-18).
