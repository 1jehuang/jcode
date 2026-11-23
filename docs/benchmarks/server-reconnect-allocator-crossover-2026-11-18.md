# Is a smaller allocator thread cache a free memory saving?

**Decision (November 18, 2026): keep the existing allocator settings.** A smaller cache helped some concurrent reconnect workloads, but did not establish a consistent resident-memory benefit for one session and increased the CPU cost of small allocation bursts. This does not change the daemon’s fresh-start memory target.

The private reconnect fixture from `server-reconnect-memory-2026-11-18.md` changed only the child daemon's `GLIBC_TUNABLES`. Runs were sequential, with a fresh process and private state each time. The fake provider verified the full 1 MiB tool result and prior answers. All 792 turns across 14 runs streamed and persisted exactly. The CPU metric below is the daemon's user plus system CPU from a ready baseline through five seconds after disconnect, not wall time. Memory is anonymous PSS at that final sample, not allocator in-use or virtual size.

| Binary / workload | Cache count | CPU seconds | Anonymous PSS, MiB |
| --- | --- | ---: | ---: |
| Selfdev `ea079a394`, 8 sessions × 8 waves | default | 13.69 | 54.57 |
| Same | 0 | 15.14 | 51.83 |
| Same | 2 | 12.77, 13.92 | 49.80, 47.23 |
| Pinned distribution build, 16 sessions × 6 waves | default | 6.35, 6.77 | 28.13, 27.29 |
| Same | 2 | 6.04, 6.57 | 21.89, 26.37 |
| Same distribution build, 1 session × 6 waves | default | 0.73, 0.49 | 19.24, 18.89 |
| Same | 2 | 0.52, 0.69 | 19.56, 18.37 |

Selfdev order was default, 0, 2, 2, 0, default. The final two runs overlapped another local job and are excluded from the performance table, but passed correctness. Distribution order was 2, default, default, 2 for each workload, on the same `c0961cf3…` executable. At one session the allocator's in-use count fell in both comparisons, but this did **not** translate into consistently fewer resident pages. Neither single-session nor high-concurrency results establish improved latency.

**CPU falsification check.** A separate `cc -O2 -pthread` probe used 160,000 iterations per thread. Each iteration allocated a batch of 32-, 64-, 128-, or 256-byte objects (one size per iteration), wrote a byte to each, then freed them in reverse. For eight threads and seven objects per batch, summed thread CPU per allocation/free pair was 4.20–4.26 ns (default), 25.37–25.43 ns (count two), and 32.16–32.33 ns (off), in mirrored order. This is a counterexample to a cost-free change in allocation behavior, **not** a claim that daemon requests become six times slower. Scratch artifacts: `allocator-crossover-20261118.py`, `allocator-cache-burst.c`, `broader-allocx-{1..6}` and `broader-allocdist-{1..8}`.

**Separate history findings.** Trimming the 5,000-event queue's unused capacity saved 740,544 requested bytes per full queue, but increased saturated insertion CPU by 5.19% and 5.28% in two hosted runs. Keeping a spare slot shifted the penalty to filling the queue. Neither change shipped. Streaming diagnostic query payloads one at a time did pass the CPU and allocation gates without cutting the history; see `server-event-query-stream-2026-11-18.md`.
