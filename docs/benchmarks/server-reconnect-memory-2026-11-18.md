# Repeated reconnect memory, November 18, 2026

## Question and controls

Does memory pile up as the same eight sessions, each with a 1 MiB tool result, repeatedly reconnect, complete one turn, disconnect, and reload persisted history in one server process? This differs from many turns on eight constantly connected sessions. Scratch fixture `ram-fork/reconnect-cycle-20261118.py` extends `broader-runtime-probe.py`: close all clients after each wave, wait 1.5 seconds, assert zero live sessions, connected clients, provider-cache bytes and tool-result bytes, sample, then restore the same session IDs. The loopback endpoint checks the tool payload and all preceding answers on every request. At the end check each persisted answer exactly once in the base file and journal. Each process uses a private home, TMPDIR, socket and test flag. These runs do not compare CPU or RSS across concurrently executing fixtures.

## Same normal binary, two allocator configurations

For `b36c547da`, both 32-wave runs passed all **256 streamed and 256 persisted answers each**. Both recorded the same 1,280 retained diagnostic events. A separate default 16-wave run passed 128 answers. These are allocator-reported in-use MiB, **not** estimates of leaked or exclusively application-owned memory:

| After disconnect | Default | Thread cache disabled in fixture only |
| --- | ---: | ---: |
| Cycle 1 | 6.36 | 4.52 |
| Cycle 8 | 10.61 | 5.22 |
| Cycle 16 | 12.91 | 5.60 |
| Cycle 31 | 15.04 | 6.19 |
| Cycle 32 + 5 seconds | 15.15 | 6.22 |
| Cycle 32 + 35 seconds | 12.52 | 6.12 |

Both had 27 threads at cycle 31 and 20 at the last sample. The diagnostic-only `GLIBC_TUNABLES=glibc.malloc.tcache_count=0` substantially changes the slope on the **same executable**, but does not remove it. This supports an allocator-cache contribution, not an attribution of every residual byte, a permanent plateau, or a production tuning recommendation. Debug formatting-length estimates totaled about 399 KiB in each final run; these estimates are not actual heap sizes.

Separately, a pinned historical jemalloc profiling binary (`3d312fee…`, not the normal binary) measured a sampled live difference of +1,223,459 bytes between restoring the same eight sessions after 1 and 15 turns. About 262,231 bytes of that difference were attributed to the swarm event recorder. The samples also include the expected larger persisted conversations, parser caches and transport allocations. This supplementary profile is neither an idle zero-session snapshot nor a substitute for the normal-binary results. Raw artifacts: `broader-reconnect16-a`, `broader-reconnect-default32`, `broader-reconnect-notcache32`, `broader-reconnect-prof16` under the scratch fixture root.

## Action

The retained diagnostic history is intentional. What was not needed was allocating a formatted `String` for each historical event solely to find its byte count on every `server:memory` request. `e6ab9efab` counts the unchanged UTF-8 Debug output without retaining it. This change targets temporary allocation and CPU **while inspecting** the daemon, not baseline RSS, the retained history, or the reconnect residual. The standalone allocation/elapsed-time probe is `scripts/probe_swarm_event_bytes.sh`. In a hosted optimized build of the actual daemon crate, the UTF-8 oracle and alternating Linux thread-CPU probe both passed (receipt `20261118-145417502-3114384`). Median time per history: common 29.46 → 18.84 µs, escaped 227.00 → 188.76 µs, large 661.45 → 424.30 µs. This measures the estimator, not end-to-end diagnostic latency.
## Post-change runtime check

On the rebuilt `a06e8df45` daemon, the isolated `broader-reconnect-verified-counter` run verified all 32 streamed and persisted answers across four waves on eight sessions. The fixture reported no provider errors. This checks runtime behavior, not a change in idle RAM.
