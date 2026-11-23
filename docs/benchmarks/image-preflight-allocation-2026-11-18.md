# Image preflight allocation and CPU evidence, 2026-11-18

Follow-up to `96df37f2b` (sharing the preflight base64 decode). This measures only
preflight decisions, not the full outbound request, pixel decode, resize, or
encoding. No further production optimization is introduced.

## Reproduction and isolation

```sh
justrust check -p jcode-base --no-default-features --test image_preflight_probe
justrust test -p jcode-base --no-default-features --release \
  --test image_preflight_probe isolated_probe -- --ignored --nocapture
```

The dedicated integration target includes the actual image-clamp source and
uses its test-only preflight wrapper. Its baseline preserves the pre-sharing
short-circuit expression (byte limit, dimension probe, media sniff). The
existing frozen equivalence matrix remains in that included module. Each
measured fixture also asserts a fixed expected decision for both versions.
The no-image fixture measures the empty preflight guard, not message traversal.

The target installs its own `System` allocator with thread-local counters, never
a second allocator in the shared base unit-test binary. It separately measures
10 calls per fixture, reporting allocation/reallocation request counts and
requested bytes. These are allocator calls and cumulative requested bytes, not
peak resident memory or allocator-internal overhead. Fixture generation,
assertions, logging and CPU clock reads are outside the counted region.

CPU samples use Linux `CLOCK_THREAD_CPUTIME_ID`, not elapsed wall time. Six paired
rounds reverse baseline/shared order on alternate rounds. Allocation counting
is disabled for these samples, though the counting allocator's disabled-path
check remains in both versions. Inputs and outputs are black-boxed. The
repository's `release` profile uses opt-level 1, with dependency overrides.

## Results

Hosted run `20261118-053706217-2633507` passed on the hosted Linux worker.
Final validation run `20261118-053915523-2646612` also passed all 20 tests:
19 image behavior/oracle tests plus the isolated probe with allocator assertions.
That validation ran tests concurrently, so CPU numbers below come from the
first, probe-only run instead.
The table reports **per preflight invocation**. Allocation-call counts are now
asserted by the ignored probe, including zero for no image.

| Fixture | Allocator calls, before → after | Requested bytes, before → after |
| --- | ---: | ---: |
| No image | 0 → 0 | 0 → 0 |
| Ordinary 100×100 PNG | 2 → 1 | 1,266 → 633 |
| Large 1400×1400 noisy PNG | 2 → 1 | 11,763,828 → 5,881,914 |
| Malformed base64 | 2 → 1 | 24 → 12 |

Median of six actual thread CPU samples per implementation, reported for the
whole iteration batch (not per invocation):

| Fixture | Iterations/sample | Before, ms | After, ms |
| --- | ---: | ---: | ---: |
| No image | 100,000 | 0.088735 | 0.088770 |
| Ordinary | 100,000 | 89.565 | 45.109 |
| Large | 40 | 281.217 | 140.523 |
| Malformed | 100,000 | 8.562 | 4.334 |

The three image cases halve allocator requests and cumulative requested bytes,
with approximately half the measured preflight CPU in this probe. The empty
guard is unchanged within noise. These are not full-request speedups, and
halving cumulative allocation does not establish a peak-RSS reduction because
the two baseline temporary buffers were sequential. The allocator here is
`System`, not the application's optional jemalloc configuration.
