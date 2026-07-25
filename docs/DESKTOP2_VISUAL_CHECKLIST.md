# Desktop2 Visual & Interface Checklist

What "good visuals" means for `jcode-desktop2`, and how each rule is enforced.

This is a working checklist, not a style essay. The design language itself
lives in `~/jcode-website/STYLE.md` (print, one ink, JetBrains Mono); this
document covers the things that actually break in a rendered app, plus the
test that catches each one.

**Rule: a checklist item is only real if something fails when it is violated.**
Every enforced row below has been mutation-tested: the rule was deliberately
broken and the named test failed. Rows marked `manual` are honest gaps.

## How to check

```sh
# everything mechanical: source lints + fast invariants
scripts/desktop2_visual_check.sh
scripts/desktop2_visual_check.sh --gpu   # also run pixel tests

# geometry + text invariants only (fast, no GPU)
cargo test -p jcode-desktop2

# pixel-level visual invariants (renders offscreen, needs a GPU)
cargo test -p jcode-desktop2 -- --ignored

# render every state-space node to PNGs for eyeballing / agent review
cargo build --profile selfdev -p jcode-desktop2 --bin jcode-desktop2
./target/selfdev/jcode-desktop2 --capture all /tmp/d2caps
```

`--capture` renders at 2x so reviewed frames match what a HiDPI window shows.

---

## 1. Resolution and scale

The single highest-value category: this is where the first cut actually broke.

| # | Rule | Enforced by |
|---|------|-------------|
| 1.1 | All layout is expressed in **logical** units, never physical pixels. | `layout::tests::layout_is_scale_independent_in_logical_units` |
| 1.2 | Text is laid out and rasterized at **physical** size, so glyphs are crisp and correctly sized at any DPI. | `visual_tests::text_is_rasterized_at_physical_size` |
| 1.3 | Hairlines are exactly **one physical pixel**, never a scaled-up blur. | `layout::tests::hairlines_are_one_physical_pixel` |
| 1.4 | The same logical window looks identical at 1x, 1.5x, 1.75x, 2x, 3x. | `layout::tests` sweep over `SCALES` |
| 1.5 | Scale changes at runtime (moving to another monitor) re-lay out. | manual: `WindowEvent::ScaleFactorChanged` |

## 2. Layout and space

| # | Rule | Enforced by |
|---|------|-------------|
| 2.1 | Body copy is confined to a **measure column** (<= 720px); long lines are unreadable. | `layout::tests::column_never_exceeds_measure` |
| 2.2 | The column is centered with balanced gutters that shrink gracefully on narrow windows. | `column_is_horizontally_balanced`, `column_stays_inside_the_window` |
| 2.3 | Regions have a strict vertical order and **never overlap**. | `regions_are_ordered_and_never_overlap`, `visual_tests::nothing_draws_in_the_gap_above_the_composer` |
| 2.4 | Nothing is drawn in the margins or off-paper; text wraps rather than clipping at the window edge. | `visual_tests::margins_stay_empty` |
| 2.5 | Degenerate windows (0-sized, extreme aspect ratios) never panic or invert geometry. | `degenerate_sizes_do_not_panic_or_invert` |
| 2.6 | Space is a design element: rhythm constants live in `layout.rs`, never inline in scene code. | `scripts/desktop2_visual_check.sh` |

## 3. Typography

| # | Rule | Enforced by |
|---|------|-------------|
| 3.1 | One family (JetBrains Mono) with a fallback stack, declared once in `text.rs`. | `scripts/desktop2_visual_check.sh` |
| 3.2 | Body leading 1.65; captions carry 0.1-0.2em letterspacing. | `layout::BODY_LEADING`, caption styles |
| 3.3 | Single-line fields **elide**, never wrap past their own rule. | `tests::elide_*`, `visual_tests::masthead_rule_is_clear_of_text` |
| 3.4 | Elision keeps the informative ends (head and tail of paths, ids, errors). | `tests::elide_respects_budget_and_keeps_ends` |
| 3.5 | Sentence case; product names keep their own casing (`jcode` lowercase). | manual |

## 4. Color and contrast

| # | Rule | Enforced by |
|---|------|-------------|
| 4.1 | Scene code speaks **semantic roles** (`text`, `muted`, `rule`, `wash`), never literal colors. | `scripts/desktop2_visual_check.sh` |
| 4.2 | Body text is dark enough to read against paper. | `visual_tests::body_text_has_readable_contrast` |
| 4.3 | Hierarchy comes from ink density: `text` > `muted` > `faint` > `rule`. | `theme::tests::ink_densities_are_ordered` |
| 4.4 | Every role is visible against its background in both modes. | `every_role_differs_from_the_background`, `both_modes_are_defined_for_every_role` |
| 4.5 | Dark mode follows the system preference. | manual: `from_env` currently defaults light |

## 5. State coverage

| # | Rule | Enforced by |
|---|------|-------------|
| 5.1 | Every visual state is an **enumerable node**, renderable without a window. | `states::NODES`, `--capture` |
| 5.2 | Visual invariants are asserted across **all** nodes, not just the happy path. | `visual_tests` iterate `states::names()` |
| 5.3 | Empty states say what to do, in `faint` ink. | `attached_empty` node |
| 5.4 | Long content degrades by scrolling/eliding, never by overlapping. | `nothing_draws_in_the_gap_above_the_composer` (`streaming`, `turn_done`) |
| 5.5 | Errors are legible and complete enough to act on. | `error` node + elision keeps the tail |
| 5.6 | Busy states are visible without spinner theatre. | `streaming` node |

## 6. Interaction (current gaps)

Honest status: desktop2 is a starter. These are not yet enforced and are the
next work.

| # | Rule | Status |
|---|------|--------|
| 6.1 | Caret is a real caret with a blink, not a typed `_`. | **gap** |
| 6.2 | Text editing supports arrows, word motion, home/end, and selection. | **gap** |
| 6.3 | Transcript scrolls (wheel, keyboard, and follows the tail). | **gap** |
| 6.4 | Copy/paste via the system clipboard. | **gap** |
| 6.5 | Focus is visible, and Escape does not silently quit the app. | **gap** (Escape currently exits) |
| 6.6 | Input remains responsive while a turn streams. | partial |
| 6.7 | Window remembers its size and position. | **gap** |

## 7. Performance and correctness

| # | Rule | Status |
|---|------|--------|
| 7.1 | Redraw is event-driven (`ControlFlow::Wait`), not a busy loop. | enforced in `main` |
| 7.2 | Text layout is not rebuilt for unchanged content every frame. | **gap** (no layout cache yet) |
| 7.3 | Font and layout contexts are created once and reused. | `TextSystem` |
| 7.4 | Dropped/suboptimal surface frames are skipped, not fatal. | `render.rs` |

## Adding a rule

1. Write the rule as one sentence describing an observable property.
2. Write the test. Prefer `layout.rs` (pure geometry, fast) over pixels.
3. **Break the code on purpose and watch the test fail.** If it passes, the
   test does not encode the rule; fix the test before trusting the row.
4. Add the row with its test name, or mark it `manual`/`gap` honestly.
