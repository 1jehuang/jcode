# Wire-buffer capacity experiment, 2026-11-18

## Outcome

**Neither candidate is a pure efficiency improvement. Leave production
`request_compression.rs` unchanged.** This probe records rejected experiments,
not a new request-body implementation or a general proof that all possible
capacity strategies lose.

Attribution motivating the experiment: 32 active sampled requests retained
approximately 80 MiB of serialization capacity for 32.1 MiB of exact wire data.
`serde_json::to_vec` starts with 128 bytes and lets `Vec` grow geometrically.
For chunked string writes the resulting slack depends on write boundaries,
not simply the next power of two above final length.

Candidates:

* `Shrink`: unchanged `serde_json::to_vec`, then `shrink_to_fit`.
* `Growth`: one serialization through an `io::Write` adapter with exact
  reservations growing by 1.5x instead of Vec's ordinary growth.

No counting pass, network requests, daemon, or user session storage is involved.
Production serialization errors, compression decisions, and upload sampling
remain unchanged because **no production code is changed**. The probe additionally
checks exact fixture bytes and the growth adapter's error category, message,
line/column, and one-pass behavior after a serializer emits a partial body.

## Measurement method

Linux x86_64, Intel Core Ultra X9 388H, CPU affinity 2. The standalone manifest
pins serde_json 1.0.149, matching the application's lockfile at measurement time.
Only serde_json `Value` fixtures are measured, not provider request construction,
compression, logging, HTTP dispatch, or full request lifecycle latency.

Build with the standalone package's default release profile, **opt-level 3**.
Do not use the application's `selfdev` profile for this measurement: it sets
opt-level 0 for local code. Initial unoptimized exploratory output was discarded.

CPU results use `CLOCK_THREAD_CPUTIME_ID`, three warmup pairs, 15 measured pairs
per candidate per fixture, and reverse baseline/candidate execution order each
pair. Two complete runs were taken. Each sample serializes approximately 16 MB,
with iteration count clamped to 8..100,000. Allocation instrumentation is compiled
out of the CPU executable, not just disabled at runtime.

A separately built `allocation-meter` executable instruments System allocator
calls. Fixture creation and exact-byte checks happen outside its measured region.
`calls` includes alloc and realloc. `requested` sums requested allocation/realloc
sizes, **not bytes copied, RSS, or net heap consumption**. `retained` and `peak`
are logical requested live bytes. A realloc's old and new blocks are not counted
as simultaneously live, so these figures do not establish allocator transient
copy peaks. Destruction of the returned buffer occurs after metering is disabled.

## CPU results

Median candidate / baseline thread CPU ratio. Lower is better. Raw pairs are
in `results/cpu-1.txt` and `results/cpu-2.txt`.

| Fixture | Shrink run 1 | Shrink run 2 | Growth run 1 | Growth run 2 |
| --- | ---: | ---: | ---: | ---: |
| tiny | 1.48703 | 1.45694 | 2.37628 | 2.29706 |
| escaped | 1.00501 | 1.00232 | 2.42814 | 2.53191 |
| nested | 1.00677 | 1.00410 | 1.77079 | 1.75053 |
| noisy | 0.99829 | 1.00464 | 1.15440 | 1.16430 |
| large | 1.00633 | 1.00660 | 1.09385 | 1.09280 |

The unconditional shrink has a reproducible 46-49% tiny-body CPU regression.
The sub-percent large-body shrink difference alone is not sufficient evidence
of a meaningful CPU regression. The lower-growth writer has clear reproducible
CPU regressions across all fixture categories. The adapter adds capacity checks
to serializer writes and may perform more reallocations. This experiment does
not separate those contributions.

## Allocation results

Raw counters are in `results/alloc.txt`. All quantities below are bytes except
calls. Every candidate produces the exact same byte length as baseline.

| Fixture | Exact length | Baseline capacity | Shrink capacity | Growth capacity | Calls baseline / shrink / growth |
| --- | ---: | ---: | ---: | ---: | --- |
| tiny | 63 | 128 | 63 | 128 | 1 / 2 / 1 |
| escaped | 396,033 | 524,288 | 396,033 | 425,553 | 13 / 14 / 21 |
| nested | 175,604 | 262,144 | 175,604 | 224,916 | 12 / 13 / 19 |
| noisy | 1,073,732 | 2,097,152 | 1,073,732 | 1,534,452 | 15 / 16 / 24 |
| large | 1,099,188 | 1,392,384 | 1,099,188 | 1,411,326 | 9 / 10 / 14 |

Shrinking releases retained capacity but always adds one realloc and increases
cumulative requested bytes. The 1.5x writer is not even a universal retained
capacity win: the large fixture grows from 1,392,384 to 1,411,326 bytes, while
allocation calls increase from 9 to 14. The escaped fixture's cumulative requested
bytes increase from 1,048,448 to 1,276,407 despite lower retained capacity.

These candidates therefore fail the no-CPU/allocation-regression acceptance gate.
A conditional shrink is measured separately below. A different allocator, other
growth factors, and a generic size hint were not tested. Do not extrapolate this
rejection to those strategies.

## Conditional large shrink follow-up

Policy: serialize once with the unchanged serializer, then shrink only when
length is at least 65,536 bytes and unused capacity is at least one quarter of
capacity. The comparison uses integer division, explicitly covered by boundary
tests. This isolated policy is **not shipped**: it offers retained-memory savings,
but does not meet the strict no-CPU/allocation-regression gate.

The same optimized, allocator-uninstrumented CPU harness ran two complete sets
of 21 alternating pairs on CPU 2. Raw data is `results/conditional-cpu-{1,2}.txt`.
The allocation-meter binary ran separately twice, with identical counters in
`results/conditional-alloc-{1,2}.txt`.

| Fixture | CPU ratio run 1 | CPU ratio run 2 | Capacity baseline -> conditional | Calls baseline -> conditional |
| --- | ---: | ---: | ---: | ---: |
| tiny | 1.02831 | 1.02784 | 128 -> 128 | 1 -> 1 |
| escaped | 1.00063 | 0.99949 | 524,288 -> 524,288 | 13 -> 13 |
| nested | 0.99503 | 1.00389 | 262,144 -> 175,604 | 12 -> 13 |
| noisy | 1.00003 | 0.99670 | 2,097,152 -> 1,073,732 | 15 -> 16 |
| large | 0.99998 | 0.99991 | 1,392,384 -> 1,392,384 | 9 -> 9 |
| below-threshold | 0.99978 | 0.99951 | 131,066 -> 131,066 | 3 -> 3 |
| at-threshold | 0.99981 | 1.00111 | 131,068 -> 65,536 | 3 -> 4 |
| above-threshold | 1.00173 | 0.99988 | 131,070 -> 65,537 | 3 -> 4 |
| many-small | 0.99585 | 0.99157 | 1,048,576 -> 768,904 | 14 -> 15 |
| native-8x1MiB | 0.99978 | 0.99348 | 16,777,760 -> 8,389,090 | 6 -> 7 |

Threshold fixtures have exact wire lengths 65,535 / 65,536 / 65,537. The native
shape has eight user messages with 1 MiB text each, content-block arrays, model,
and stream fields. It is a constructed request-shaped `Value`, not an actual
provider type or network workload. Many-small has 20,000 short user messages.
Escaped and large do not meet the 25% slack rule and therefore do not shrink.

The native-shaped payload releases 8,388,670 logical retained bytes per buffer,
about 8 MiB. This is meaningful retained-capacity evidence, not evidence that
all capacity reductions are impossible. Large-body CPU results do not establish
a regression. However, the tiny fixture reproducibly costs about 2.8% more CPU
despite skipping shrink, and every triggered shrink adds one realloc. Native
cumulative requested bytes increase from 32,507,038 to 40,896,128; logical peak
remains 16,777,760. Allocator RSS, copying behavior, and whole-lifecycle latency
are not established by these counters. This policy is a memory/CPU/allocation
tradeoff, not a validated pure efficiency improvement.

Exact-byte and policy-boundary tests pass. Existing partial serialization error
and single-pass tests still pass. Compression and sampling remain unchanged
because production code was not edited. No counting serialization was added.

Reproduce follow-up after building/copying the CPU executable as below:

```bash
taskset -c 2 "$JCODE_SCRATCH_DIR/wire-cpu" cpu-conditional 21
taskset -c 2 "$JCODE_SCRATCH_DIR/wire-cpu" cpu-conditional 21
# Build the separate allocation-meter executable before these commands:
examples/wire_capacity_probe/target/release/wire-capacity-probe alloc-conditional
examples/wire_capacity_probe/target/release/wire-capacity-probe alloc-conditional
```

## Reproduce

Run from the repository root. Choose an allowed CPU with `taskset -pc $$`.
The commands below deliberately copy the CPU executable before building the
allocation-feature variant, since the latter replaces the target executable.

```bash
justrust check --manifest-path examples/wire_capacity_probe/Cargo.toml --all-targets
justrust test --manifest-path examples/wire_capacity_probe/Cargo.toml --release
justrust build --manifest-path examples/wire_capacity_probe/Cargo.toml --release
cp examples/wire_capacity_probe/target/release/wire-capacity-probe "$JCODE_SCRATCH_DIR/wire-cpu"
taskset -c 2 "$JCODE_SCRATCH_DIR/wire-cpu" cpu 15
taskset -c 2 "$JCODE_SCRATCH_DIR/wire-cpu" cpu 15
justrust build --manifest-path examples/wire_capacity_probe/Cargo.toml --release --features allocation-meter
examples/wire_capacity_probe/target/release/wire-capacity-probe alloc
```

Hosted justrust builds were used. Only the returned standalone executable ran
on this laptop. Production files were not edited and no daemon reload is needed.
