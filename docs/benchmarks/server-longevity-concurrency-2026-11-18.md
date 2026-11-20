# Bounded server longevity and 32-session concurrency acceptance, 2026-11-18

## Scope and outcome

This is a local, no-inference acceptance exercise of the pinned normal runtime
`runtime-20261118-owned-after`, not a before/after performance comparison and not
an hours-long soak test. The workload extends the earlier
`runtime-acceptance-probe.py` methodology using a new dedicated scratch fixture.
The original fixture and runtime sources were not edited by this lane.

Both final acceptance runs passed: 128 sequential turns in one session and eight
waves of 32 simultaneously outstanding requests, totaling 384 exact streamed
answers and 384 independently checked persisted answers. Session, payload,
provider-cache, and client owners were released after disconnect. Allocator live
bytes did not return to startup, so this is not a claim of leak freedom or a
fully explained long-term plateau.

## Fixture and isolation

All fixture files live below `/home/jeremy/.jcode/scratch/ram-fork/broader-*`.
Each invocation creates a new directory, refuses to reuse an existing one, and
launches a private daemon with all of the following:

- `JCODE_HOME=<work>/home`, `JCODE_TEST_SESSION=1`.
- `JCODE_RUNTIME_DIR=<work>/run` and an explicit private main/debug socket pair.
- `TMPDIR=<work>/tmp`, to isolate the background-task registry too.
- `JCODE_TEMP_SERVER=1`, private owner PID, telemetry disabled, memory feature
  disabled, tool profile `none`, and no update/self-development launch.
- A loopback HTTP fake native Responses provider, dummy API key, and an explicitly
  local provider base URL. No remote inference is used.

The daemon executable is verified via the listener PID and `/proc/<pid>/exe`.
The existing synthetic scratch session template is reused only for metadata.
All messages are replaced with fixture content. Each session starts with one
exactly 1 MiB tool result, including a distinct two-digit session prefix.
The runtime's model label is just a fixture routing identifier.

The fake provider checks the native `/v1/responses` route, model, exact tool
output, session identity, wave identity, absence of duplicate requests, and every
prior assistant answer in the next request. For each wave, every session's HTTP
request must arrive before **any** response is released. This proves 32 provider
requests are simultaneously outstanding, not merely 32 connected idle clients.

Responses contain a deterministic session/wave-specific string split over two
SSE deltas. Every wire client concatenates the deltas and asserts exact equality
before accepting its terminal `done`. Final assertions independently verify each
answer appears once in persisted assistant messages, merging the base JSON with
its append journal. The fixture also asserts no live session, connected client,
provider-cache JSON, retained tool payload, or background task remains at the end.

### Reproduction on the measurement host

The dedicated fixture is `broader-runtime-probe.py`. Its dependencies are Python's
standard library, Linux `/proc`, `ss`, the pinned binary, and the existing synthetic
`before-1/home/sessions` template. The script and raw results are deliberately
scratch artifacts, not committed runtime or test-suite changes.

```bash
cd /home/jeremy/.jcode/scratch/ram-fork
python3 broader-runtime-probe.py broader-longevity128-final-20261118 \
  --sessions 1 --waves 128 --seed-kib 1024 --disconnect-waits 0,5,35,70
python3 broader-runtime-probe.py broader-concurrent32-final-20261118 \
  --sessions 32 --waves 8 --seed-kib 1024 --disconnect-waits 0,5,35,70
python3 broader-summarize.py \
  broader-longevity128-final-20261118 broader-concurrent32-final-20261118
```

Use new `broader-*` labels when repeating those commands. The report's final runs
are sequential, so the two fixture daemons do not share the pinned executable's
file-backed PSS with each other.

Pinned executable SHA-256:

```text
8417f8f6304ac5e734624f747524893045705832b2a602b56732c6ab315bfc3a
```

Original, unmodified `runtime-acceptance-probe.py` SHA-256:

```text
cc21d44af51f9d961efd1cde14f194df732c1c01a3e2b097c023b33b3df5e674
```

Final `broader-runtime-probe.py` SHA-256:

```text
68da195c060815a40a0b89b1487b9defbd730f8b9b6ecf98c853d81bf89591ff
```

## Measurement definition

MiB means 1,048,576 bytes. Allocator live bytes are the daemon's reported
`server:memory-incident.process.allocator_live_bytes`; this normal executable
reports the `system` allocator, not a profiling build. `MALLOC_CONF=prof:false`
is recorded for parity with the earlier fixture, not as proof of jemalloc use.
RSS/PSS are read directly from the daemon's `/proc/<pid>/smaps_rollup`. CPU is
cumulative user plus system CPU time from `/proc/<pid>/stat`, not wall time or
whole-machine utilization. Thread count comes from `/proc/<pid>/task`.

Each selected wave is sampled with all provider requests blocked, then after all
exact outputs complete. Other samples cover baseline, attachment before turns,
all completed while clients remain attached, and 0/5/35/70 seconds after clients
close both sockets and their buffered stream handles. Full owner accounting is
captured after completion and after the final disconnect wait.

These are single snapshots rather than repeated statistical medians. Debug
commands themselves allocate and record diagnostic snapshots. `/proc` and debug
reads are adjacent, not atomic. Reported CPU includes diagnostics and lifecycle
work, but excludes the Python fake provider and clients. No throughput or latency
speedup is claimed.

## Results

### One session, 128 sequential turns

Raw artifact: `broader-longevity128-final-20261118/results.json`.
All 128 requests and 128 exact streamed answers passed. All 128 persisted
answers passed the independent base-plus-journal check. Final owner-release
assertions passed and the fixture exited successfully.

| Stage | elapsed s | Live MiB | RSS MiB | PSS MiB | CPU s | threads | live sessions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| baseline | 3.33 | 2.549 | 91.059 | 87.224 | 0.10 | 20 | 0 |
| attached before turns | 3.39 | 4.218 | 105.133 | 100.322 | 0.14 | 21 | 1 |
| active turn 1 | 3.49 | 7.880 | 112.648 | 107.838 | 0.27 | 21 | 1 |
| completed turn 4 | 4.71 | 6.580 | 127.809 | 122.996 | 0.60 | 21 | 1 |
| completed turn 32 | 7.96 | 7.577 | 122.074 | 117.262 | 3.16 | 21 | 1 |
| completed turn 64 | 11.02 | 8.019 | 122.246 | 117.436 | 5.61 | 21 | 1 |
| active turn 128 | 17.07 | 12.087 | 125.555 | 120.744 | 10.39 | 22 | 1 |
| all completed, attached | 17.35 | 8.601 | 123.641 | 118.830 | 10.42 | 22 | 1 |
| disconnected, 0 s | 17.37 | 9.649 | 123.660 | 118.850 | 10.42 | 22 | 0 |
| disconnected, 5 s | 22.35 | 7.115 | 122.730 | 117.920 | 10.45 | 22 | 0 |
| disconnected, 35 s | 52.35 | 7.033 | 121.492 | 116.682 | 10.46 | 20 | 0 |
| disconnected, 70 s | 87.36 | 7.026 | 120.570 | 115.758 | 10.47 | 20 | 0 |

The attached session retained exactly 1,048,576 tool-result bytes and 259 canonical
messages. Provider-cache JSON and tool-result bytes were zero. By final disconnect
accounting, live sessions, canonical messages, payloads, and provider caches were
all zero. Thread count returned to its baseline of 20. Cumulative CPU rose only
0.05 seconds over the 70-second disconnect observation interval.

Live allocation still ended 4.477 MiB above baseline and completed-turn snapshots
grew between turns 4 and 128. This is not attributed solely to transcript growth:
the explicit remaining event-history owner is only 85,250 estimated bytes, and
canonical payload text grew by only 11,936 bytes beyond the seed. This workload
therefore verifies owner release but does **not** fully explain the residual or
prove a long-term allocator plateau.

### 32 sessions, eight simultaneous-request waves

Raw artifact: `broader-concurrent32-final-20261118/results.json`.
All 256 requests and exact streamed answers passed, with 32 requests held
simultaneously at the local provider in every wave. All 256 persisted answers and
final owner-release assertions passed. The sequential two-run command exited 0.

| Stage | elapsed s | Live MiB | RSS MiB | PSS MiB | CPU s | threads | live sessions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| baseline | 3.23 | 2.552 | 91.262 | 87.438 | 0.07 | 22 | 0 |
| attached before turns | 3.86 | 48.527 | 154.918 | 150.139 | 0.79 | 22 | 32 |
| active wave 1 | 5.09 | 147.270 | 242.855 | 238.075 | 3.06 | 35 | 32 |
| completed wave 1 | 5.40 | 47.902 | 172.469 | 167.687 | 3.22 | 35 | 32 |
| active wave 2 | 6.56 | 149.137 | 239.168 | 234.387 | 4.98 | 35 | 32 |
| completed wave 2 | 6.85 | 48.682 | 175.203 | 170.422 | 5.13 | 35 | 32 |
| active wave 4 | 9.25 | 150.584 | 240.844 | 236.064 | 8.80 | 35 | 32 |
| completed wave 4 | 9.54 | 49.801 | 176.906 | 172.126 | 8.94 | 35 | 32 |
| active wave 8 | 14.38 | 152.478 | 243.434 | 238.653 | 16.40 | 35 | 32 |
| all completed, attached | 14.70 | 51.289 | 179.699 | 174.920 | 16.56 | 35 | 32 |
| disconnected, 0 s | 14.71 | 52.298 | 179.828 | 175.049 | 16.57 | 35 | 31 |
| disconnected, 5 s | 19.70 | 11.489 | 147.734 | 142.954 | 17.04 | 35 | 0 |
| disconnected, 35 s | 49.71 | 10.890 | 143.988 | 139.209 | 17.05 | 21 | 0 |
| disconnected, 70 s | 84.70 | 10.865 | 143.992 | 139.210 | 17.08 | 20 | 0 |

While attached after all answers, accounting retained 33,554,432 canonical
tool-result bytes, 608 messages, and no provider-cache tool-result/JSON bytes.
At final disconnect accounting, all live sessions, canonical messages, payloads,
provider caches, clients, subscription channel/membership counts, and background
tasks were zero. Shutdown-signal count fell from 32 to zero. Cleanup is
asynchronous: the immediate disconnect snapshot still saw 31 live sessions,
but the five-second sample saw none. Threads fell from 35 to 20 by 70 seconds,
below the startup sample's transient count of 22.

The final active-versus-completed wave delta was about 101.2 MiB of live
allocation. Post-disconnect allocation was 10.865 MiB, still 8.313 MiB above
baseline, with 143.992 MiB RSS and 139.210 MiB PSS. The final full memory report
also reported roughly 16.3 MiB allocator-retained bytes. None of those residuals
is hidden by reporting only session payload totals. CPU rose 0.51 seconds during
the 70-second disconnect interval, mostly in its first five seconds.

The post-disconnect debug event deque retained 608 events, estimated at 199,058
bytes. This is the explicit retained owner discussed below, but is much smaller
than the total allocator residual.

## Retained-owner investigation

The workload deliberately retains its initial tool payload while attached.
Successful disconnect cleanup must release that owner rather than trim or change
the transcript to manufacture lower memory usage.

A concrete growing owner is the server's debug/swarm event-history deque. The
final 32-session run retained 608 events, estimated at 199,058 bytes, after
all live sessions disappeared. This is an intentional diagnostic owner, not a
retained 32 MiB tool transcript.
`crates/jcode-app-core/src/server/swarm_event_history.rs` pushes an event and pops
the oldest when exceeding `MAX_EVENT_HISTORY`, which is 5,000 in
`crates/jcode-app-core/src/server/state.rs`. Thus the observed count growth has a
source-level count bound. This test does not establish a byte bound for arbitrary
notification/file-detail strings,
and does not justify changing diagnostic retention without a separate workload.

The active-versus-completed allocator delta remains a concrete follow-up target:
request construction and transport state hold substantially more live memory
while 32 requests are blocked before response headers. The fixture reproduces
that exact phase without inference. Normal-process counters do not identify
allocation stacks, so attributing the entire delta to a specific builder, buffer,
or duplicate would be speculation. This report does not propose an unvalidated
source fix or claim a leak from that transient peak.

RSS need not return to startup merely because live sessions disappear. Allocator
retention, faulted executable pages, caches, diagnostic history, and runtime
threads are separate owners. A nonzero post-disconnect residual is reported, not
silently treated as transcript leakage or proof of leak freedom.

## Fixture corrections and caveats

1. The smoke test and an initial repeated-turn run did not set `TMPDIR`. Although
   homes and sockets were private, the daemon reported 1,576 global background
   task records, totaling 1,148,593 serialized bytes. They are not final evidence.
   The fixture now isolates `std::env::temp_dir()/jcode-bg-tasks` through private
   `TMPDIR` and requires the final registry count to be zero. No global task files
   were cleaned, deleted, or otherwise manipulated by this lane.
2. An initial persisted-output assertion checked only the base JSON and failed
   after 128 successful streamed answers. Inspection showed the answers were
   correctly stored in `*.journal.jsonl`; the base snapshot was intentionally
   unchanged. The fixture was corrected to merge append records and the final
   workloads rerun. That was a fixture assertion error, not evidence of runtime
   answer loss.
3. Earlier overlapping exploratory runs are not used for final PSS comparisons.
   Sharing the same binary inode changes proportional file-backed accounting.
4. The final workloads have small deterministic answers and no executed tools,
   real model reasoning, cancellation, reconnect/resume, or compaction. They
   cover repeated history preservation and 32 in-flight requests, not all
   long-lived production-session behavior.
5. One 128-turn session and eight 32-session waves are bounded acceptance cases,
   not a multi-hour soak or a proof that every retained owner is bounded. No
   source changes are made based on a merely unexplained residual.

## Follow-up: concurrent request peak allocation stacks

The previously unexplained active-request peak now has a concrete stack-level
attribution. This is an attribution run, not a normal-versus-profiling speed or
memory comparison. No runtime source was changed or rebuilt by this exercise.

The dedicated fixture gained `--binary` and `--profile`. Profiling asserts the
pinned executable digest, requests dumps using the existing attached
`session_broader_00`, and sets a private final-profile prefix. The same private
home, runtime directory, temporary directory, test-session flag, wire assertions,
history assertions, persisted base-plus-journal checks, and teardown are retained.

```bash
cd /home/jeremy/.jcode/scratch/ram-fork
python3 broader-runtime-probe.py broader-concurrent32-profile-20261118 \
  --binary "$PWD/owned-history-20261118-prof-jcode" --profile \
  --sessions 32 --waves 2 --seed-kib 1024 --disconnect-waits 0,5,35,70
python3 broader-profile-summary.py
```

Pinned binary SHA-256:
`3d312fee01b1ada46dab7863a26f16a042600068b4ecf7132a6652f881b605b1`.
Updated fixture SHA-256:
`0a0d523a4a46c0c79e88b8f84898c4c139f68559ebe2124f4b4ca25a8329fd41`.
The earlier fixture digest above describes the earlier normal-binary acceptance.
The profiling environment is
`prof:true,prof_active:true,lg_prof_sample:15,prof_accum:true,prof_final:true`
plus `prof_prefix:<private-work>/postdisconnect` in `MALLOC_CONF`.

The bounded run exited zero: 64 exact streamed answers, 64 independently verified
persisted answers, no provider errors, 32 requests held concurrently in each of
two waves, and all final owner-release assertions passed. Wire request bodies
were 1,050,490 through 1,050,744 bytes. The daemon exited and emitted
`postdisconnect.2662604.0.f.heap`. This final dump occurs during graceful process
exit after the 70-second disconnect observation, not at the exact instant of the
last debug snapshot.

### Live versus cumulative attribution

Each heap was analyzed with `jeprof --text --show_bytes <binary> <heap>` and
again with `--alloc_space`. Reports are beside their heaps under the private
`broader-concurrent32-profile-20261118` directory. Cumulative values measure all
sampled allocation traffic up to that stage, including freed and reallocated
buffers. They must not be added to live values or treated as retained memory.
Function cumulative columns also overlap their callees.

| Stage | sampled live bytes | cumulative allocated bytes |
| --- | ---: | ---: |
| active wave 1 | 168,567,479 | 958,462,625 |
| active wave 2 | 169,638,518 | 1,460,092,024 |
| both waves completed, attached | 47,064,435 | 1,472,195,533 |
| final process-exit dump | 5,423,706 | 1,583,457,018 |

The active wave-2 sampled live owners include:

- **83,886,080 bytes (80 MiB)** below
  `jcode_provider_core::request_compression::json_body`, specifically
  `serde_json::ser::to_vec`, string serialization, and buffer growth. The same
  allocation path is 251,756,547 cumulative bytes after two waves. These are
  serialization allocation stacks, not proof of a second hidden transcript.
- **33,752,646 bytes** below
  `jcode_provider_openai::request::build_responses_input_with_logger`, the
  native Responses JSON input owner. Its cumulative traffic is 134,713,244
  bytes after two waves. The `serde_json::value::to_value` child is part of this
  owner and is not another independent 32 MiB to add.
- **33,587,424 bytes** below restored canonical session-history loading, still
  live when attached after completion. This is the intentional 32-session seed
  history, not a candidate for deleting transcript content.

Both request-building and wire-serialization owners disappear from the attached
completed live profile. The sampled active-to-completed drop is 122,574,083
bytes. Those two request paths explain 117,638,726 bytes of it, about 96%.
The canonical history owner then disappears from the final exit profile too.
This identifies the principal phase-specific owners without claiming every
small diagnostic/runtime allocation has been exhaustively explained.

### Concrete next implementation target

`crates/jcode-provider-core/src/request_compression.rs:293` serializes with
`serde_json::to_vec`. The compressed branch already calls `shrink_to_fit` at
line 219, while the raw branch moves the untrimmed `Vec` into the wire body at
lines 311 and 331. This local custom endpoint correctly does not compress.
The actual combined wire content is about 32.1 MiB, but its sampled backing
allocations occupy 80 MiB. Roughly 47.9 MiB is capacity beyond content at this
phase, including allocator size-class rounding. The body remains live while the provider has read the request but has
not returned headers. That capacity is a concrete redundant allocation target,
not required history or an inherently necessary extra copy of payload text.

The narrow next candidate is compacting excessive raw wire-buffer capacity
before handing it to reqwest, preserving exact bytes and retry behavior.
It requires tests covering uncompressed/custom endpoints, eligible compression,
and request replay, followed by the same isolated 32-request runtime check.
No reduction, speedup, or safe production fix is claimed until that experiment
is implemented and validated. The request `Value` passed by reference through
`crates/jcode-provider-openai-runtime/src/openai_stream_http.rs:7,68-80` is a
separate native-input owner. Removing it would require analyzing retry and
fallback semantics, not simply dropping the canonical transcript.

### Profiling caveats and residuals

Process allocator counters in this profiling build were 187,644,552 bytes at
active wave 2, 66,173,912 attached after completion, and 25,226,088 after 70
seconds disconnected. RSS/PSS at those stages were 297.547/292.583,
278.137/273.173, and 190.605/185.643 MiB. CPU rose from 5.36 to 5.94 seconds
during disconnect observation and threads returned from 35 to 20. Profiling
metadata, allocator implementation/capacity classes, diagnostic timing, and
sample estimation prevent direct equality with sampled heaps or the earlier
normal system-allocator numbers. The final exit heap also follows teardown.

The final sampled live heap has no large transcript/request owner. Its remaining
small owners include broadcast-channel storage (917,504 bytes cumulative on that
live stack), runtime/task state, parsers, regex caches, and TLS/library state.
These observations are not a complete attribution of the normal build's earlier
post-disconnect residual, nor proof of an hours-long plateau or global
optimization exhaustion. One initial jeprof batch exceeded its 240-second
symbolization budget, so unfinished reports were regenerated with a longer
budget. The daemon workload itself passed on its first profiling invocation.
