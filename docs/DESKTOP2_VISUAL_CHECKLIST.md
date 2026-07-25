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

# list the keybindings ported from the TUI, and what was skipped
./target/selfdev/jcode-desktop2 --keys

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
| 2.7 | The footnote row is reserved even when empty, so a notice never shifts the composer. | `regions_are_ordered_and_never_overlap`, `layout_is_scale_independent_in_logical_units` |
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
| 5.7 | A node renders identically regardless of when it is rendered. | `visual_tests::state_nodes_render_deterministically` |
| 5.8 | Interaction states are nodes too (caret mid-text, blink off, scrollback, notice). | `states::NODES` |

## 6. Interaction

The composer is a real input box, and the keybindings are ported from the TUI
so muscle memory transfers. `keymap::PORTED` is the parity table: each row
names a chord, its action, and the TUI binding it mirrors, and
`every_ported_chord_resolves` asserts the chord really resolves through the
same code path the app uses. `keymap::NOT_PORTED` lists TUI chords that were
deliberately skipped, with the reason.

| # | Rule | Enforced by |
|---|------|-------------|
| 6.1 | The caret is a real insert bar drawn at the cursor, not a typed `_`. | `visual_tests::an_insert_caret_is_drawn_in_the_empty_composer` |
| 6.2 | The caret tracks the cursor index, so text is inserted where the caret is. | `the_caret_moves_with_the_cursor`, `editor::tests::insertion_happens_at_the_cursor_not_the_end` |
| 6.3 | The caret is solid while typing and blinks once idle. | `caret::tests::caret_is_solid_immediately_after_typing`, `caret_blinks_once_idle` |
| 6.4 | Blinking is scheduled, never a busy redraw loop. | `caret::tests::blink_is_scheduled_rather_than_polled`, `scheduled_toggle_actually_flips_visibility` |
| 6.5 | The caret never escapes the composer well, at any size. | `layout::tests::the_caret_always_fits_inside_the_composer`, `visual_tests::the_caret_stays_inside_the_composer_well` |
| 6.6 | **Escape never quits.** It interrupts, then clears, then re-follows the tail. | `keymap::tests::escape_cancels_and_never_quits`, `action_tests::escape_clears_the_input_instead_of_quitting` |
| 6.7 | Ctrl+C interrupts while busy and quits only when idle **and** empty. | `action_tests::ctrl_c_quits_only_when_idle_and_empty` |
| 6.8 | Emacs motion and word motion work (Ctrl+A/E/B/F, Alt+B/F, Ctrl/Alt+arrows). | `keymap::tests::every_ported_chord_resolves`, `action_tests::editing_chords_reach_the_editor` |
| 6.9 | Word semantics match the TUI exactly. | `editor::tests::word_motion_matches_the_tui_semantics` |
| 6.10 | Kill/cut/word-delete work (Ctrl+U/K/W/X, Alt+D, Alt/Cmd+Backspace). | `keymap::tests::all_word_delete_aliases_resolve` |
| 6.11 | Every edit is undoable; no-ops do not consume undo. | `editor::tests::undo_restores_text_and_cursor_for_every_edit`, `no_op_edits_do_not_push_undo_states` |
| 6.12 | Cut and paste round-trip through the system clipboard. | `action_tests::cut_then_paste_round_trips_through_the_clipboard`, `clipboard::tests` |
| 6.13 | Up/Down recall submitted input and restore the live draft. | `editor::tests::history_recall_round_trips_and_restores_live_input` |
| 6.14 | The transcript scrolls, clamps at both ends, and follows the tail. | `action_tests::scrolling_clamps_and_returns_to_the_tail` |
| 6.15 | Submitting returns to the live tail so the reply is visible. | `action_tests::submitting_jumps_back_to_the_live_tail` |
| 6.16 | Multi-byte text and emoji never split or panic. | `editor::tests::multibyte_text_never_splits_a_char`, `emoji_deletes_as_one_unit` |
| 6.17 | Plain typing is never swallowed by a shortcut. | `keymap::tests::plain_typing_is_not_captured_as_a_shortcut` |
| 6.18 | Ctrl and Cmd are interchangeable for editing chords. | `keymap::tests::cmd_and_ctrl_are_interchangeable_for_editing` |
| 6.19 | No key can panic the app, on any model state. | `action_tests::every_action_is_safe_on_an_empty_model`, `every_ported_chord_dispatches_without_panicking` |
| 6.20 | A no-op action explains itself instead of failing silently. | `action_tests::submitting_without_a_session_keeps_the_text_and_says_why` |
| 6.21 | Tests never read or clobber the developer's real clipboard. | `clipboard::tests::tests_never_touch_the_real_system_clipboard` |

Remaining interaction gaps, honestly:

| # | Rule | Status |
|---|------|--------|
| 6.22 | Mouse text selection and click-to-position-caret. | **gap** |
| 6.23 | Shift+arrow keyboard selection. | **gap** |
| 6.24 | Multi-line composer (Shift+Enter currently inserts a space). | **gap** |
| 6.25 | Window remembers its size and position. | **gap** |
| 6.26 | Slash-command autocomplete, queue mode, stash (see `NOT_PORTED`). | **gap** |

## 7. Performance and correctness

| # | Rule | Status |
|---|------|--------|
| 7.1 | Redraw is event-driven (`ControlFlow::Wait`), not a busy loop. | enforced in `main` |
| 7.2 | Text layout is not rebuilt for unchanged content every frame. | **gap** (no layout cache yet) |
| 7.3 | Font and layout contexts are created once and reused. | `TextSystem` |
| 7.4 | Dropped/suboptimal surface frames are skipped, not fatal. | `render.rs` |
| 7.5 | Caret blinking wakes on a scheduled instant, never a spin loop. | `caret::tests::blink_is_scheduled_rather_than_polled` |

## Adding a rule

1. Write the rule as one sentence describing an observable property.
2. Write the test. Prefer `layout.rs` (pure geometry, fast) over pixels.
3. **Break the code on purpose and watch the test fail.** If it passes, the
   test does not encode the rule; fix the test before trusting the row.
4. Add the row with its test name, or mark it `manual`/`gap` honestly.
