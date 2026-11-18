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
A conditional shrink, a different allocator, other growth factors, and a generic
size hint were not tested. Do not extrapolate this rejection to those strategies.

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
