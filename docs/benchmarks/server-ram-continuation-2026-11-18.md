# Server RAM continuation, November 17, 2026 (Pacific)

## Where the previous round stopped

The November 16 pricing-parser change was completed and activated. The paired
headless workload fell from 171.56 MiB to 109.23 MiB median server PSS before
the retained-heap watchdog ran. A 130-second control converged to approximately
98–99 MiB on both versions. This was a transient refresh-retention improvement,
not a demonstrated reduction in the long-term idle floor. See
[the prior measured report](server-headless-pss-2026-11-17.md).

A read-only production sample at 05:14 UTC November 18 found 15 live sessions,
14 connected clients and four running agents. `server:memory-incident` reported
349,445,120 bytes PSS, 247,659,184 allocator-live bytes and 73,684,304 estimated
resident retained bytes. These are a busy multi-session observation, not a
single-session regression against the earlier headless benchmark. Concurrent
activity visibly changes these values. No sessions were stopped or production
allocator purge requested.

## Constant-space fallback cache tracking

`91f4e9cfd` removes the temporary `Vec<u64>` of every prefix hash in
`CacheTracker::record_request`. Only the full hash and the hash at the previous
message boundary are consumed, so those two scalar values are sufficient.
The hash operations and cache-violation state machine are unchanged.

The removed vector requested `8 * message_count` bytes on this platform per
fallback call. This is a structural allocation removal, not a measured PSS or
latency reduction. The main agent path already uses cached prefix snapshots
when available, so the claim is specifically about its fallback path.

Hosted `justrust check -p jcode-base` passed. All seven cache-tracker tests
passed, including a 40-step comparison against the materialized-prefix method
checking both full tracker state and returned violations after empty input,
growth, mutation, shrink, reset and bounded-history rollover. Hosted test run:
`20261117-211807639-218789`.

## Selected compressed request buffers

`e806ead83` and followup `1dfdbd8cf` compact the completed compressed buffer
only after the existing size, endpoint, savings and zstd expansion-limit checks
select it for sending. Both gzip and zstd are covered. Rejected/noisy bodies
keep the original raw fallback and do not pay the new shrink cost. The raw
JSON buffer and codec settings are unchanged.

For an approximately 8 MiB deterministic conversation fixture:

| Encoding | Previous returned capacity | Selected compacted capacity | Median encode time before | Median encode time after |
| --- | ---: | ---: | ---: | ---: |
| gzip | 4,194,370 bytes | 2,367,473 bytes | 24.743 ms | 24.555 ms |
| zstd | 8,421,508 bytes | 2,059,758 bytes | 68.818 ms | 68.855 ms |

The explicit benchmark rotates fixture/policy order and measures twelve rounds
after warmup. These are encoder microbenchmarks on a shared host, not
end-to-end latency guarantees. Small timing differences are noise-scale and do
not prove a speedup or zero overhead. A selected buffer may incur a realloc
and copy. The peak allocation while encoding is unchanged. Capacity savings
are not evidence that all those pages were resident or that idle server PSS
falls by the same amount. The zstd conversation releases about 6.07 MiB of
retained allocation capacity while the request body is owned by transport.

Hosted check and the full provider-core package suite passed: 209 tests passed,
three ignored. Tests compare exact compressed bytes against the original
encoders, decompress large accepted bodies, check capacity, preserve raw and
expansion-limit fallbacks, and send real TCP uploads to verify headers and wire
bytes. Reproduction:

```bash
justrust check -p jcode-provider-core
justrust test -p jcode-provider-core
justrust test -p jcode-provider-core --lib gzip_allocation_benchmark -- --ignored --nocapture
```

## Remaining ownership disposition

Two source passes checked agent state, provider state, registry sharing and
request preparation. Useful canonical transcripts, frozen prompt/tool snapshots,
retry inputs and active session state are deliberately retained. Registry tools
and skill registries are already shared with `Arc`. The transcript JSON totals
exclude busy sessions and do not measure all backing allocations, so the gap
between those totals and allocator-live bytes is not by itself wasted memory.

The earlier scratch transport study established capacity savings but did not
establish a consistent physical RAM reduction. This round accepts explicit
selected-buffer allocation compaction with measured encoding-time limitations,
not a new whole-process resident RAM claim. Raw JSON buffer shrinking, more
aggressive allocator trimming, cache eviction and shorter history retention
are not part of this change.

## Integration and runtime verification

Hosted clippy passed with warnings denied for both touched packages:
`justrust clippy -p jcode-provider-core -p jcode-base --lib -- -D warnings`
(run `20261117-212227290-228383`). The coordinated TUI selfdev build completed
successfully on hosted (run `20261117-212441537-235056`), but its publication was
correctly marked superseded because concurrent unrelated checkout edits changed
the source fingerprint before activation. The production daemon was not forcibly
reloaded or repointed around that safety check.

The successful artifact was pinned independently as
`~/.jcode/scratch/server-ram-20261118-candidate-jcode`, SHA256
`ce9748cbc0a1861647dbcfc6db938deb2def424702fa4467fece5a5c1a6bf4f1`.
It reports `v1.0.52-dev (0b24f81f0)` and includes unrelated concurrent work,
so it is not a clean memory-only build. The isolated profiler completed startup,
headless session creation, settled idle and private-only explicit purge with no
model calls. Dedicated homes, test-session flags, runtime directories and sockets
kept these checks out of user history. Artifacts are
`~/.jcode/scratch/server-ram-20261118-before` and `server-ram-20261118-after`.

The old shared executable sampled 36,355 KiB settled PSS and the independently
pinned candidate 103,700 KiB. These are lifecycle smoke samples, **not comparable
PSS acceptance measurements**: executable-page sharing differs, startup refresh
completion was not synchronized, and source changes are not isolated. No new
idle-floor or production-resident-memory reduction is claimed. The scoped
changes are committed and pushed, with activation left to the next source-stable
coordinated rebuild/reload rather than bypassing build identity protection.
