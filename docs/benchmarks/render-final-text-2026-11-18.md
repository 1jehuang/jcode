# Final rendered text ownership, 2026-11-18

## Scope

At the end of each stored message, move the sole nonempty text or reasoning
String into the rendered row. Preserve `format!("{}{}", reasoning, text)` when
both are nonempty. Compute user-prompt classification against the original text
before moving either buffer. Earlier ToolResult and ProviderNative flush paths
retain their existing clear/reuse policies. There is no cache or API change.

The independent original renderer is frozen from dbfe72162,
`crates/jcode-base/src/session/render.rs:369-726`. A textual comparison confirmed
only two parent-session paths are qualified for the test child module.
Parity compares every serialized output field, including image anchors and
prompt ordinals. Fixtures cover empty/tiny/large plain text, reasoning-only,
mixed, images with and without text, labels assembled across text fragments,
suppressed attached-image labels, and buffers left behind after either flush.

## Preliminary debug validation (not optimized acceptance)

- Hosted `justrust check -p jcode-base --tests` passed.
- 11 renderer tests passed with full persisted reasoning enabled.
- Isolated rebuilt test binary's CPU/allocation probe passed all seven cases.
- `git diff --check` passed.

Measurements below are from the local x86_64 Linux debug test binary on the
user's Intel Core Ultra X9 388H. The renderer test build inadvertently routed
locally because wrapping justrust in a dedicated `JCODE_HOME` hid its hosted
account configuration. The preceding check ran on hosted successfully. For
subsequent measurements the already-built executable ran directly, with
`JCODE_HOME` set only on that executable, `JCODE_TEST_SESSION=1` and
`JCODE_REASONING_DISPLAY=full`. No synthetic sessions were saved in normal
user storage. Keep compiler account environment separate from test isolation
when reproducing.

CPU uses actual `CLOCK_THREAD_CPUTIME_ID`, not elapsed time. After 64 warmups,
12 rounds alternate old/new execution order. The table uses sorted sample 6
of 12 for each implementation. Empty/tiny/no-text use 20,000 iterations per
sample, other cases 500. Allocation counting is per-thread and performed
separately, disabled for CPU measurement. Counts include allocation/reallocation
calls and requested bytes for one complete render. Both implementations share
the same counting allocator and immutable input session.

| Case | CPU new/old | Old CPU ns/sample | New CPU ns/sample | Allocation calls old/new | Requested bytes old/new |
| --- | ---: | ---: | ---: | ---: | ---: |
| empty | 0.9465 | 5,665,948 | 5,362,855 | 0 / 0 | 0 / 0 |
| tiny | 0.9665 | 60,621,619 | 58,589,133 | 6 / 5 | 1,389 / 1,381 |
| plain-large | 0.5805 | 5,403,528 | 3,136,759 | 6 / 5 | 525,661 / 263,517 |
| reasoning-only | 1.0052 | 4,457,228,002 | 4,480,202,497 | 228 / 227 | 2,376,451 / 2,114,295 |
| mixed | 0.9667 | 4,742,221,708 | 4,584,100,626 | 230 / 230 | 3,162,907 / 3,162,907 |
| images | 0.6082 | 7,858,140 | 4,778,951 | 10 / 9 | 526,259 / 264,115 |
| no-text | 0.9113 | 34,996,530 | 31,892,180 | 5 / 5 | 630 / 630 |

All cases satisfy the declared CPU nonregression gate of 1.05 and strictly
nonincreasing allocation calls and bytes. Reasoning markup dominates its
fixture, so the reasoning-only result establishes no material CPU regression,
not a speedup. Empty/no-text paths allocate no combined String in either
implementation, so their small CPU differences should not be generalized.
The meaningful plain-large and image-text reductions remove one 256 KiB copy
and allocation per final rendered text buffer.

The ignored test is
`session::render::image_context_tests::final_text_tests::final_text_cpu_allocation_probe`.
Run it alone with `--ignored --nocapture --test-threads=1` on the rebuilt test
executable using the isolated environment above. Normal frozen parity is
`final_text_frozen_parity`.

## Optimized hosted acceptance

The debug measurements above do not substitute for optimized acceptance.
Both probes were rerun on the hosted x86_64 Linux machine using the repository's
release profile (`opt-level = 1`). No production or test changes were required.

Receipts:

- `20261118-045454995-2494200`: hosted release compilation and both independent
  frozen renderer parity tests passed (2 passed, 2 measurement tests ignored).
  `justrust show` reports `where hosted on hosted`. The saved compiler command
  for `jcode_base` includes `-C opt-level=1`, including its test compilation.
  Release test executable: `jcode_base-7796e47604c4ddd7`.
- `20261118-045712828-2501345`: both ignored CPU/allocation probes passed
  (2 passed, 0 failed, 0 ignored) on hosted, reusing the release executable
  (357 fresh units, zero compiled). Both tests also execute frozen parity before
  measurement. No `JCODE_HOME` override was applied to justrust or compilation.
  The Cargo target runner applies the isolated home and full reasoning mode
  only to the actual executable.

Reproduction commands:

```bash
justrust test --release -p jcode-base --lib session::render::image_context_tests -- --nocapture --test-threads=1
justrust test --release -p jcode-base --lib cpu_allocation_probe \
  --config 'target.x86_64-unknown-linux-gnu.runner = ["env", "JCODE_HOME=/tmp/jcode-render-release-20261118", "JCODE_TEST_SESSION=1", "JCODE_REASONING_DISPLAY=full"]' \
  -- --ignored --nocapture --test-threads=1
justrust show 20261118-045454995-2494200
justrust show 20261118-045712828-2501345
```

The same 64 warmups, 12 alternating old/new rounds, actual per-thread CPU
clock and separately counted allocations described above apply. CPU entries
are sorted sample 6 of 12, in nanoseconds per batch, not per render. The hosted
machine was busy (13.4 other cores on average, up to 64 other rustc processes),
so these are CPU-time comparisons rather than wall-time speedup claims.
The host differs from the local laptop used for preliminary debug results.

### Final text release results

| Case | CPU new/old | Old CPU ns/sample | New CPU ns/sample | Allocation calls old/new | Requested bytes old/new |
| --- | ---: | ---: | ---: | ---: | ---: |
| empty | 0.9535 | 915,250 | 872,710 | 0 / 0 | 0 / 0 |
| tiny | 0.8738 | 3,625,700 | 3,168,280 | 6 / 5 | 1,389 / 1,381 |
| plain-large | 0.3748 | 2,782,510 | 1,042,950 | 6 / 5 | 525,661 / 263,517 |
| reasoning-only | 0.9967 | 369,711,740 | 368,506,290 | 174 / 173 | 2,370,561 / 2,108,405 |
| mixed | 1.0033 | 515,600,930 | 517,290,080 | 176 / 176 | 3,157,017 / 3,157,017 |
| images | 0.4866 | 2,223,380 | 1,081,930 | 10 / 9 | 526,259 / 264,115 |
| no-text | 0.8291 | 2,366,600 | 1,962,230 | 5 / 5 | 630 / 630 |

### Image context release results

This probe compares the frozen renderer from `7e8ca3a2f` with the current
renderer, so its result includes both image-context ownership and the final-text
change. Its tool/image fixtures allocate the same calls and bytes as the earlier
image-context-only debug probe. It is not a standalone final-text attribution.

| Case | CPU new/old | Old CPU ns/sample | New CPU ns/sample | Allocation calls old/new | Requested bytes old/new |
| --- | ---: | ---: | ---: | ---: | ---: |
| empty (noimages) | 0.9876 | 432,830 | 427,460 | 0 / 0 | 0 / 0 |
| tiny | 0.7365 | 10,589,710 | 7,799,720 | 41 / 30 | 4,170 / 3,453 |
| largeinput | 0.5918 | 4,924,750 | 2,914,540 | 41 / 30 | 790,578 / 527,725 |
| images | 0.7062 | 4,944,560 | 3,491,790 | 47 / 36 | 791,185 / 528,332 |

All eleven optimized cases pass the declared CPU ratio gate of 1.05 and
nonincreasing allocation calls and requested bytes. Reasoning-only and mixed
establish nonregression, not a CPU speedup. No reproducible regression was
observed, so both renderer candidates are accepted under this synthetic gate.
This does not claim an interactive TUI or end-to-end provider speedup.
