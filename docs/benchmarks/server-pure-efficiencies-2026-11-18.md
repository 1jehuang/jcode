# Server pure-efficiency pass, November 18, 2026

## Acceptance contract and iteration

Remove redundant computation and ownership without reducing history, changing prompt/cache identities, evicting useful data, changing provider wire semantics, or deliberately exchanging CPU for RAM. Exact legacy oracles and optimized alternating measurements gate changes. Allocated bytes are not resident RAM, retained capacity is not RSS, and cumulative call stacks overlap.

The earlier gzip/zstd finished-buffer shrinking changes are **not** counted as pure-efficiency wins: they may realloc/copy. This pass makes no claim of a lower process RAM floor.

Explicit todo done gates repeatedly returned **NOT DONE**. Initial ownership review found hashing, discarded diagnostics, telemetry-only JSON buffers and filtered identity clones. Deeper cumulative profiling found response-output normalization clones. Real-cache review then found duplicate search-index normalization. Those findings caused implementation and more measurement rather than an early completion claim.

## Accepted changes

| Commit | Removed work and compatibility | Observed result |
| --- | --- | --- |
| `9df7f6b9f` | Stream stable message hashes without payload Vec. Same FNV bytes, metadata identities, serialized lengths and cache invalidation | Two optimized alternating cold runs: CPU 1.2–63% lower. A 1 MiB message allocated 3,146,204 → 201 cumulative bytes. 46 tests passed. |
| `fcb98e734` | Session-only server memory attribution instead of discarded per-agent process/interrupt/frozen snapshots and JSON clones. Same full session attribution and busy-lock behavior | 64 populated profiles: 26.312 → 3.419 ms. 12 filtered tests passed. One server process snapshot remains. Instrumentation-only difference: server:memory no longer inserts redundant synthetic agent:memory history samples. Standalone agent diagnostics unchanged. |
| `14a964693` | Borrow filtered withheld-call identity input. Same top-level filters, map order and serialized String hash framing, nested nulls unchanged | 16 alternating optimized rounds, eight shapes: small 177.2 → 67.0 ns, 9 → 3 allocations. Large 79.653 → 52.111 us, 929 → 13 allocations, 275,324 → 138,090 cumulative bytes. Hosted check and legacy oracle passed. |
| `812a0be89`, `51ea20709` | Serialize only the hook environment prefix. Same UTF-8 truncation and intentionally possibly incomplete JSON prefix | Small 273 → 108 ns; plain 4 MiB 1.305 → 1.054 ms; escaped 9.630 ms → 30.8 us; nested 13.461 ms → 33.1 us. Large allocations 12.6–16.8 MB → 16.6–32.7 KB. Serde still scans unescaped strings before emitting them. |
| `51ea20709` | Count lifecycle telemetry JSON bytes rather than allocate discarded String, borrow sorted key names | Hosted release 24-round CPU medians: small 85 → 20 ns; many fields 29,662 → 8,170 ns; 1 MiB ASCII 267,754 → 233,149 ns; escaped Unicode 1,516,835 → 251,679 ns. Output capacity up to 2,097,176 bytes → zero. Three optimized parity/probe tests passed. |
| `17439beb7` | Move selected response output on final emission, use indices before then. Preserve duplicate emissions, ordering, missing/real selection and counters | Full package: 69 passed, two ignored. Small 560 → 359 ns, many 79.432 → 37.540 us. Normalizing 16 × 1 MiB: 5.275 ms → 9.870 us and 33,582,138 → 5,070 allocated bytes. Fixture construction/result destruction excluded. Not an end-to-end request speedup. |
| `553037b22` | Exact Value-only browser budget counts without output buffers, same trimming/omission and wire output | Hosted browser tests: 110 passed, seven ignored. Ten alternating CPU rounds: new/old medians 0.1050 small, 0.1145 many, 0.1974 large. Zero counter allocations versus legacy 4/120 bytes, 960/28,800 bytes, 16/524,280 bytes per workload pass. Fallible typed request guard unchanged. |
| `b2ddd72e0` | Fallible stack count for startup usage snapshot size preflight, unchanged private atomic writer | Root actual Linux thread CPU: 581-byte snapshot 950.3 → 325.2 ns; 50,111-byte snapshot 70,670.3 → 25,713.1 ns. Eight snapshot tests passed, including exact 65,535/65,536/65,537-byte boundaries, serializer/non-UTF8 errors, permissions, symlinks and reader limits. |
| `44c58e8c2` | Lowercase index text once, deduplicate borrowed tokens instead of duplicate owned strings | Ordinary query split-then-lower behavior unchanged. Index retains whole-text lowercase before splitting, preserving combining-dot and contextual-sigma behavior. Exhaustive Unicode scalar oracle and index hash/overflow oracle passed. Original alternating elapsed probe improved 25–49% across five cases with fewer allocations. Actual thread-CPU followup recorded below. |

Dedicated test modules retain legacy oracles and ignored benchmark recipes. Timing thresholds are not normal CI requirements. Measurements use the optimized release profile (opt-level=1), not selfdev alone (opt-level=0). Several isolated probes ran locally because hosted mirroring does not accept external scratch manifests. Primary implementation checks/tests used hosted justrust.

## Rejected candidate

A fallible count-only typed browser request preflight reduced output allocation but regressed CPU. Hosted optimized alternating Linux thread CPU for a 160 KiB plain typed request rose from **38,086 to 71,170 ns**, about 87%. The experimental API and production change were fully restored, not shipped. A final local probe using the original estimator again regressed that case, **58,238 to 77,476 ns**, despite small/many-field improvements. Buffer removal alone does not qualify.

Earlier buffered streaming SHA fingerprints also regressed CPU 16–31%. Finished compressed-buffer shrinking can copy. Neither is included among pure wins.

## Profiling methodology

A pinned `jemalloc-prof` executable ran with a private `JCODE_HOME`, `JCODE_TEST_SESSION=1`, unique socket/runtime, fake credentials and localhost native Responses fixture. Sixteen synthetic sessions each contain eight 128 KiB tool outputs. Native `/v1/responses` model/input shape was asserted. The post-fix run also asserted exact output contents, call IDs and order. Requests were held during active snapshots, then released through fixture SSE completion. No external inference, normal user session history, or shared-daemon allocator was used.

```bash
justrust build --profile selfdev -p jcode --bin jcode --features jemalloc-prof
jeprof --text --inuse_space PINNED_BINARY live-16.heap
jeprof --text --alloc_space --cum PINNED_BINARY completed.heap
```

`MALLOC_CONF=prof:true,prof_active:true,lg_prof_sample:15,prof_accum:false` for retained ownership, and `prof_accum:true` for cumulative allocation. Local fixture scripts under `~/.jcode/scratch/ram-fork/heap-stack-probe.py` and `heap-allocation-probe.py` depend on earlier local synthetic fixtures, so they are evidence artifacts, not portable repository benchmarks.

Before artifacts: `~/.jcode/scratch/ram-fork/pure-pass1-20261118/` and `pure-pass2-cumulative-20261118/`, executable `~/.jcode/scratch/pure-efficiency-20261118-prof-jcode`.
Post-fix artifacts: `~/.jcode/scratch/ram-fork/pure-pass3-after-20261118/`, executable `~/.jcode/scratch/pure-efficiency-20261118-after-prof-jcode`.

| Sampled attribution | Before | Post-fix |
| --- | ---: | ---: |
| Active application retained heap | 62.1 MiB | 62.1 MiB |
| Completed application retained heap | 24.2 MiB | 24.4 MiB |
| Cumulative allocation | 482.3 MiB | 445.5 MiB |
| Cumulative native Responses input owner | 64.6 MiB | 32.3 MiB |

This confirms reduced allocation at the response-copy owner, **not** a controlled total-memory A/B. Other source changes, samples, timing and profiling overhead differ. Instrumented 16-session private memory actually rose from about 147.2 to 201.6 MiB, allocator live counters from about 74.2 to 108.8 MB. The increase is not attributed by this experiment and must not be presented as resident-memory savings. Sampled application live state remained essentially unchanged. Stack totals overlap and must not be added.

A separate real-daemon metadata snapshot attributed about 37.1 MB canonical JSON to 11 lockable sessions, with three other sessions contended, and about 12.3 MB search indexes. This triggered normalization work, but is not complete process accounting or an A/B comparison.

## Final ranked owner dispositions

| Remaining owner | Disposition |
| --- | --- |
| Canonical parsed session history | Required retained state. Truncation, compaction and eviction affect semantics or future work. No further independent redundant owner identified here. |
| Native provider request tree and encoded wire body | Distinct representations used by current async transport. Output clones removed. Streaming transport redesign is an unproven future hypothesis, not a measured free win. |
| UI history rendering and protocol events | Required separate public representation. Writes already release buffers before later logging. Buffer reuse may extend retention and exact-capacity preflight adds scanning. No further validated free change identified. |
| Search indexes | Hash arrays already shared by Arc and reused. Smaller Bloom filters change false positives/future CPU. Duplicate normalization removed with exact index parity. |
| Cache fingerprints | Previous buffered SHA experiment rejected for CPU regression. No new validated alternative here. |
| Pricing and fixed infrastructure | Cache already Arc-shared. No redundant retained payload identified. Allocator purging/arena tuning/compression are not assumed tradeoff-free. |
| Diagnostics and size-only buffers | Redundant work removed only where measurements support it. Typed browser exception retained after CPU rejection. |

The final scope is **no further identified and validated no-tradeoff candidate among these examined owners**, not proof that no optimization exists anywhere. No lower RSS floor has been established. New workloads or ownership hypotheses should reopen the explicit profile, implement, measure and done-gate loop.

## Final validation followup

Hosted final tokenizer suite: **15 passed**, including the ignored optimized probe and exhaustive Unicode oracle. Actual Linux thread-CPU medians, 15 alternating rounds:

| Workload | Legacy ns | New ns | Allocations old → new | Allocated bytes old → new |
| --- | ---: | ---: | ---: | ---: |
| Small ASCII | 364 | 290 | 15 → 8 | 499 → 375 |
| Small Unicode | 815 | 663 | 16 → 9 | 588 → 464 |
| Many tokens | 254,525 | 200,073 | 4,022 → 2,022 | 355,754 → 273,360 |
| Large ASCII | 1,234,148 | 698,483 | 52,005 → 10 | 376,620 → 148,546 |
| Large Unicode | 3,097,777 | 2,061,671 | 52,006 → 11 | 784,620 → 528,554 |

CPU decreased 19–43% across the tested cases. The test allocator adds equal instrumentation to both variants. Reproduce with `justrust test -p jcode-session-types --release --lib -- --include-ignored --nocapture --test-threads=1`. Hosted receipt: `20261118-012747626-1613318`.

Final local integration during two hosted sync outages passed six browser size/parity/probe tests, eight index tests, and 25 background tests. A later lint attempt hit a justrust compiler/artifact mismatch rather than a source error. Once hosted service recovered, final production-library clippy passed for app-core, base, message-types, provider-openai, provider-core and session-types, and the hosted optimized index suite passed all eight tests (receipt `20261118-012911063-1616606`). Earlier whole-test lint encountered unrelated baseline deny-lints and is not represented as a passing whole-workspace lint run. Implementation package results above are completed evidence, not a claim of a whole-workspace test run.

Coordinated TUI build/reload completed at 09:31 UTC on November 18. Both current and shared-server channels resolved to `bdbc5643d-dirty-f987e9baca35`, with the unrelated working changes preserved. `selfdev status` reported v1.0.81-dev, and `server:info` returned git hash `bdbc5643d` and a responsive server. This is runtime integration/smoke evidence, not an end-to-end resident-memory speedup claim.
