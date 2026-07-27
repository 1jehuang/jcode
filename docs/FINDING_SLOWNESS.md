# Finding slowness without waiting to feel it

The default way a performance problem gets found in a UI is that someone uses
it, notices it drag, and says so. That has three failure modes, and this
repository hit all of them at once: it only catches what a person happens to
touch, it only fires after the code has shipped, and what it produces is an
impression rather than a number you can act on or compare against tomorrow.

This note describes the alternative that desktop2 now uses, and how to apply it
elsewhere.

## The lever: a pure render function over an enumerable state space

`jcode-desktop2`'s `build_scene` is a pure function of `Model`. `states::NODES`
already enumerates the app's visual states, because the same list drives
offscreen capture for visual review.

Those two facts together mean frame cost is not something you have to *observe*
in a running window. It is a function you can *evaluate*, over a known space,
with no window, no compositor, no GPU, and no clock:

```
jcode-desktop2 --profile-states            # ranked table
jcode-desktop2 --profile-states 2000       # with a custom warm budget, in us
```

The command exits non-zero only for problems that are real regardless of the
machine: any state redoing layout on an unchanged frame, or a state past the
40ms failure threshold. The 4ms warm budget in the table is *advisory*. It is
wall clock, so an unoptimised build or a machine busy compiling something else
will flag healthy states, and a tool that cries wolf gets ignored. When rows
say SLOW but relayout is `0/n`, read the relayout column and move on.

The measurement lives in `crates/jcode-desktop2/src/profile.rs`, and runs as a
gate in `scripts/check_guardrails.sh` and in CI.

## Measure work, not just time

The sweep reports two things per state, and the second matters more.

**Wall-clock cost** (cold and warm) is what you read as a human. Cold is the
first frame with empty caches: a resize, a theme switch, the first paint. Warm
is the steady state, which is what almost every frame really is (a caret blink,
an animation tick, a scroll, one streamed delta).

Time alone makes a bad gate. A number measured while the rest of the test suite
shares the cores is worth several times its idle value, and CI runners are
slower and more variable than a developer's machine. So a timing gate has to
carry enough slack to avoid flaking, and slack is exactly the room a real
regression hides in. A gate that flakes gets muted, and a muted gate is no gate.

**Relayout count** is the fix for that. The transcript layout cache reports how
many messages a frame had to lay out from scratch. On a steady-state frame the
answer must be zero, because nothing changed. That is a statement about *work*,
so it is exact, reproducible, and identical on the fastest laptop and the
slowest shared runner. No slack required.

This distinction is not academic. When the regression that motivated all this
was reintroduced deliberately, the loose timing gate passed on some states while
the relayout gate caught every single one.

## The state space must reach the slow end

The 25 original nodes were all small, pretty screens. That is correct for visual
capture and useless for profiling: a sweep over them reports the app as fast
while a real session lags, which is precisely how the transcript relayout
shipped unnoticed.

Four `heavy_*` nodes now cover a 60-turn session, a wall of code, a wide table,
and math-heavy output. A test asserts the heavy end stays meaningfully heavier
than the median, so the coverage cannot quietly erode as nodes are added.

Note that the heavy-end assertion is made on **cold** cost. Warm cost is
deliberately flat when the caches are doing their job, so asserting a spread
there would amount to asserting the caches are broken.

## Proving the harness actually detects things

A perf harness that always prints OK is worse than none, because it is
reassuring. The only way to know it works is to break the thing it is supposed
to catch and watch it fail.

For desktop2 that is a one-line change: make the cache always miss.

```rust
// crates/jcode-desktop2/src/paint.rs, in TranscriptCache::lay_out
let reusable = false && self.keys.get(index).is_some_and(...);
```

Then `bash scripts/check_guardrails.sh --skip-slow` must go red and name the
states. Revert, and it must go green. Do this whenever the gate is changed in a
way that could weaken it.

## Applying this elsewhere

The pattern generalizes to any renderer with these two properties:

1. Rendering is a pure (or purely-derivable) function of a model.
2. The interesting models are enumerable, rather than only reachable by
   driving the real UI.

If both hold, add a sweep, add heavy nodes, gate on a work counter rather than
only on time, and prove it by breaking something.

The jcode TUI satisfies both (it has headless golden-render tests and mature
render caches in `ui_messages_cache.rs`), so the same harness would transfer.
It has not been done, because the TUI's caches are not currently carrying this
class of bug; the note is here so the option is on the shelf rather than
rediscovered.

## What this does not cover

The sweep measures scene *building*, on states someone thought to enumerate. It
does not measure GPU rasterization, compositor behaviour, input latency, or
states nobody wrote down. Runtime instrumentation (logging frames over budget
from the real app, with the state that produced them) is the natural complement:
the sweep prevents known regressions, and logging discovers unknown states worth
adding to the space. Logging alone is the weaker half, because it still needs a
person to hit the slow path first.
