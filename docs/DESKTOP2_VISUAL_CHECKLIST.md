# Desktop2 Visual & Interface Checklist

What "good visuals" means for `jcode-desktop2`, and how each rule is enforced.

This is a working checklist, not a style essay. The design language itself
lives in `~/jcode-website/STYLE.md` (print, one ink, JetBrains Mono); this
document covers the things that actually break in a rendered app, plus the
test that catches each one.

**Rule: a checklist item is only real if something fails when it is violated.**
Every enforced row below has been mutation-tested: the rule was deliberately
broken and the named test failed. Rows marked `manual` are honest gaps.

## Running it

```sh
# build (selfdev routes crates/jcode-desktop2/ changes here automatically)
selfdev build target=desktop2

# launch: ~/.local/bin/jcode-desktop2 prefers an installed binary, then the
# self-dev build, so a rebuild is picked up without reinstalling.
jcode-desktop2
```

Bound to `Alt+Shift+I` in niri, alongside `Alt+I` for the original desktop app.

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

# drive a chord sequence and print the resulting composer state. Use this for
# manual verification: synthetic input tools (wtype/ydotool) drop or remap
# modifiers and pointer coordinates on Wayland, so they are not trustworthy
# for keybinding checks.
./target/selfdev/jcode-desktop2 --script 'type:alpha beta' ctrl+a shift+right shift+right

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
| 2.8 | The input box sits on the **middle of the page** and grows symmetrically about that line; it only leaves the centre when the window is too short. | `layout::tests::the_composer_sits_on_the_middle_of_the_page`, `a_roomy_page_centres_the_composer_exactly`, `visual_tests::the_composer_well_is_drawn_on_the_middle_of_the_window` |
| 2.9 | The masthead carries the build identity: version, update state, and signed-in account. | `meta::tests::the_caption_lists_version_update_and_account`, `visual_tests::the_masthead_meta_row_is_drawn_and_legible` |
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

### Selection and pointer

| # | Rule | Enforced by |
|---|------|-------------|
| 6.22 | Clicking places the caret at the clicked character. | `action_tests::clicking_places_the_caret_at_the_clicked_character` |
| 6.23 | Hit-testing uses the frame that was actually drawn, so clicks stay correct after a resize. | `the_recorded_frame_matches_the_rendered_geometry`, `resizing_moves_the_hit_test_with_the_layout` |
| 6.24 | Hit-testing never returns a mid-character offset. | `action_tests::hit_testing_never_splits_a_character` |
| 6.25 | Clicking after a trailing space lands after it, not before. | `action_tests::hit_testing_maps_x_to_the_nearest_character_gap` |
| 6.26 | Dragging selects text, in either direction. | `dragging_selects_the_text_between_press_and_release`, `dragging_right_to_left_selects_the_same_range` |
| 6.27 | Dragging outside the well keeps extending instead of dropping the selection. | `action_tests::dragging_above_or_below_the_well_keeps_extending` |
| 6.28 | Releasing ends the drag; later moves do not select. | `releasing_ends_the_drag_so_later_moves_do_not_select`, `a_pointer_move_without_a_press_changes_nothing` |
| 6.29 | Double click selects a word; two slow clicks do not. | `double_clicking_selects_the_word_under_the_pointer`, `two_slow_clicks_are_not_a_double_click` |
| 6.30 | Shift+click extends from the existing caret. | `action_tests::shift_clicking_extends_from_the_existing_caret` |
| 6.31 | Clicks outside the composer are ignored. | `action_tests::clicking_outside_the_composer_is_ignored` |
| 6.32 | Shift+motion extends a selection; unshifted motion collapses it. | `keymap::tests::shift_motion_extends_instead_of_moving`, `editor::selection_tests::plain_motion_collapses_the_selection` |
| 6.33 | Typing or deleting replaces the selection, undoably. | `editor::selection_tests::typing_replaces_the_selection`, `deleting_a_selection_is_undoable` |
| 6.34 | Copy and cut prefer the selection over the whole line. | `copy_prefers_the_selection_over_the_whole_line`, `cut_removes_only_the_selection_when_there_is_one` |
| 6.35 | The selection renders as a band under the text, and only when there is a selection. | `visual_tests::a_selection_is_visible_and_text_on_it_stays_readable`, `no_band_is_drawn_without_a_selection` |
| 4.6 | Selected text stays readable against the selection band, in both themes. | `theme::tests::selected_text_stays_readable` |
| 5.9 | The mouse wheel scrolls the transcript and clamps. | `action_tests::the_wheel_scrolls_and_clamps_like_the_keyboard` |

### Multi-line composer

| # | Rule | Enforced by |
|---|------|-------------|
| 6.36 | Shift+Enter inserts a real newline; Enter still submits. | `action_tests::shift_enter_makes_a_new_line_and_enter_still_submits`, `multiline_tests::shift_enter_inserts_a_real_newline` |
| 6.37 | Newlines are content; other control characters are stripped and CRLF is normalized. | `tests::control_characters_are_stripped_but_newlines_are_kept`, `tests::pasted_crlf_is_normalized` |
| 6.38 | The composer grows about its centre line as lines are added, and the transcript yields space. | `layout::tests::the_composer_grows_with_its_line_count`, `action_tests::the_composer_frame_follows_the_input_line_count` |
| 6.39 | Growth is capped, so a long paste never eats the page. | `tests::the_composer_stops_growing_at_the_line_cap`, `tests::a_tall_composer_never_eats_the_whole_page` |
| 6.40 | Home/End and Ctrl+U/K act on the current line, not the whole buffer. | `multiline_tests::home_and_end_work_on_the_current_line_not_the_buffer`, `multiline_tests::kill_to_end_stops_at_the_line_break`, `multiline_tests::kill_to_start_stops_at_the_line_break` |
| 6.41 | Up/Down move between lines, falling through to history only at the edges. | `action_tests::up_moves_between_lines_before_recalling_history`, `action_tests::down_moves_between_lines_before_returning_from_history`, `multiline_tests::line_motion_reports_when_there_is_nowhere_to_go` |
| 6.42 | Vertical motion preserves the column and clamps on shorter lines. | `multiline_tests::moving_between_lines_preserves_the_column`, `multiline_tests::moving_to_a_shorter_line_clamps_to_its_end` |
| 6.43 | Every line renders on its own row, with the caret on the cursor line. | `visual_tests::a_multiline_message_renders_on_multiple_rows` |
| 6.44 | Clicking a lower line places the caret on that line. | `action_tests::clicking_a_lower_line_lands_on_that_line` |
| 6.45 | A selection spanning a line break highlights every line it covers. | `visual_tests::a_selection_across_lines_highlights_every_line`, `multiline_tests::selection_can_span_lines` |
| 6.46 | Line motion and multi-line offsets stay on char boundaries. | `multiline_tests::line_motion_stays_on_char_boundaries` |

### Window and pointer

| # | Rule | Enforced by |
|---|------|-------------|
| 6.47 | The window reopens at the size and position it was left. | `window_state::tests::round_trips_through_the_saved_format`, `action_tests::window_geometry_is_remembered_across_resizes` |
| 6.47a | Geometry is saved as it changes, not only on a clean exit, so a crash does not lose it. | `geometry_is_saved_promptly_the_first_time` |
| 6.47b | Saving is throttled and skips unchanged values, so dragging an edge does not hammer the disk. | `repeated_saves_are_throttled_while_dragging`, `unchanged_geometry_is_not_rewritten` |
| 6.48 | A corrupt, tiny, or absurd saved geometry degrades to defaults instead of opening a broken window. | `corrupt_content_never_panics_and_falls_back`, `a_tiny_saved_window_is_rejected`, `an_absurd_saved_window_is_rejected` |
| 6.49 | Negative positions are kept (multi-monitor), non-finite ones dropped. | `negative_positions_are_kept_for_multi_monitor_setups`, `non_finite_positions_are_dropped` |
| 6.50 | The pointer is a text caret over the composer and an arrow elsewhere. | `action_tests::the_pointer_becomes_a_text_caret_over_the_composer` |
| 6.51 | The pointer shape and the click target agree exactly. | `action_tests::the_composer_hit_area_matches_the_drawn_well` |

Note: the saved size is a *request*. Tiling compositors (niri, sway) decide
window geometry themselves, so restoration is only visible for floating
windows. That is compositor policy, not a bug to fix here.

### Soft wrapping

| # | Rule | Enforced by |
|---|------|-------------|
| 6.54 | A long line wraps inside the well instead of running past its right edge. | `visual_tests::a_long_line_wraps_inside_the_composer_well` |
| 6.55 | Wrapping grows the well and is a view concern: the buffer keeps one logical line. | `action_tests::a_long_line_wraps_into_multiple_rows_and_grows_the_well` |
| 6.56 | Wrapping breaks at whitespace, and mid-word only when a word exceeds the row. | `wrap::tests::long_text_wraps_at_whitespace`, `a_word_longer_than_the_line_breaks_mid_word` |
| 6.57 | No row exceeds the width budget, at any budget. | `wrap::tests::wrapped_rows_never_exceed_the_budget` |
| 6.58 | Wrapping never loses, duplicates, or splits a character. | `wrapping_preserves_every_character`, `row_boundaries_stay_on_char_boundaries` |
| 6.59 | Rows are ordered, never overlap, and cover the whole text. | `wrap::tests::rows_are_ordered_and_never_overlap` |
| 6.60 | Every cursor offset maps to a row, so the caret can never vanish. | `wrap::tests::every_offset_maps_to_some_row`, `the_cursor_maps_to_a_row_and_column` |
| 6.64 | The caret is drawn on the row that owns the cursor, not the first row. | `visual_tests::the_caret_sits_on_the_cursor_row_when_wrapped` |
| 6.61 | The wrap budget matches the measured font width, so text cannot silently overflow. | `action_tests::the_wrap_budget_matches_the_measured_font_width` |
| 6.62 | Clicking a wrapped row places the caret on that row. | `action_tests::clicking_a_wrapped_row_lands_on_that_row` |
| 6.63 | A degenerate width never hangs or panics. | `wrap::tests::a_zero_budget_does_not_hang_or_panic` |

Remaining interaction gaps, honestly:

| # | Rule | Status |
|---|------|--------|
| 6.52 | Slash-command autocomplete, queue mode, stash (see `NOT_PORTED`). | **gap** |
| 6.53 | Selecting text in the *transcript* (only the composer is selectable). | **gap** |

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
