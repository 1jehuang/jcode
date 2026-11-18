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

## Validation

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
