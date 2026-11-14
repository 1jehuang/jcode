# Findings C — jcode-tui Pre-Existing Test Failures (Read-Only Triage)

**Workspace:** `/Users/kooshapari/.jcode/scratch/wt-resume-relend`
**HEAD:** `b99b19bc8 test(jcode-app-core): drain resume peer stream to fix test deadlock`
**Command:** `cargo test -p jcode-tui --no-fail-fast` (run via `.audit/run-tui-tests.sh` wrapper that `tee`s both live progress and full output)
**Run time:** ~508 s (mostly compile — 2480 tests, 19 ignored, 0 skipped)
**Output log:** `.audit/tui-test-output.log` (3500+ lines; panic sections at L3170–3346)
**Failures-raw list:** `.audit/failures-raw.txt` (29 entries extracted via `grep FAILED$`)

---

## Summary line

```
SUMMARY: cargo test -p jcode-tui exit=101, total=2509, passed=2480, failed=29, ignored=19
         real-bug=9, stale-expectation=19, environment=1, flaky=0
```

**Expected vs actual count:** the prompt hinted ~27 failures; actual is **29** (8 % higher than the budget, but well within the same order of magnitude). 26 of the 29 had captured panic messages in the full log; the 3 mystery failures (`overscroll_*`) needed an isolated re-run to recover their panics (now captured at lines 562, 591, 636 of `swarm_buffer.rs`).

**Run mode:** `--no-fail-fast` so all 29 ran to completion. Total wall time including cargo's first-time dep build was ~8.5 min; an incremental rerun would be <60 s for the failing subset.

**Source policy upheld:** zero source files were edited. This is a triage report only.

---

## Counts by classification

| Class               | Count | Confidence mix             | Notes                                                                 |
|---------------------|------:|----------------------------|-----------------------------------------------------------------------|
| `stale-expectation` |    19 | 17 high / 2 medium         | Tests describe old code, behavior, or string formats that no longer apply. |
| `real-bug`          |     9 | 4 high / 4 medium / 1 low  | Implementation drift or incomplete refactor; tests describe the intended behavior. |
| `environment`       |     1 | 1 high                     | Test env inherits HERDR env vars from wrapper; not a code bug.         |
| `flaky`             |     0 | n/a                        | None of the 29 failed intermittently; all deterministic.               |
| **Total**           |   **29** |                          |                                                                       |

---

## Counts by module

| Module / file                                                  | Failures | Class mix                                  |
|----------------------------------------------------------------|---------:|--------------------------------------------|
| `tui/app/helpers_tests.rs`                                     |        3 | 2 stale (rename) + 1 environment           |
| `tui/app/turn_notify.rs`                                       |        1 | stale (rename)                             |
| `tui/app/commands_accounts_01/part_01.rs` (btw/fork)           |        3 | stale (rename)                             |
| `tui/app/remote_events_reload_02/part_01.rs`                   |        1 | stale (string format)                      |
| `tui/app/remote_events_reload_04.rs`                           |        1 | stale-or-bug (cost calc)                   |
| `tui/app/remote_startup_input_03/part_01.rs`                   |        1 | real-bug (save/restore reload)             |
| `tui/app/state_model_poke_03.rs`                               |        1 | real-bug (empty-catalog activation)        |
| `tui/app/scroll_copy_01/part_01.rs`                            |        1 | stale (status-line row)                    |
| `tui/ui_tests/swarm_buffer.rs`                                 |        3 | stale (overscroll row)                     |
| `tui/session_picker_tests.rs`                                  |        8 | real-bug cluster (preview refactor)        |
| `tui/ui_messages/tests.rs`                                     |        1 | stale (rename)                             |
| `tui/terminal_mode_tests` (mod.rs)                             |        3 | stale (kitty keyboard flags)               |
| `tui/tests` (mod.rs misc)                                      |        2 | 1 real-bug + 1 stale (kitty flags)         |

---

## Top-3 concerns (in priority order)

### 1. Session-picker preview rendering cluster — 8 tests, 1 module, all real-bug

Eight tests in `crates/jcode-tui/src/tui/session_picker_tests.rs` fail with panics like:
- `preview cache built on first render` (line 2118) — `picker.preview_cache` is `None` after `render()`.
- `long preview should be scrolled down` (line 2173)
- `long preview should scroll to bottom by default` (line 2539)
- `should scroll to bottom without search` (line 2585)
- `preview scrollbar glyph should render when content overflows` (line 1944) — row returned is the raw "Test session" content with no scrollbar glyph.
- `sticky prompt header should pin a numbered prompt at the top of the preview` (line 1998) — `row=""`.
- `preview should auto-scroll to the bottom once content loads, got 0` (line 2066)
- `session list scrollbar glyph should render when list overflows` (line 1967)

**Diagnosis:** the preview-pane render path appears to have been refactored (the cache is no longer populated eagerly, sticky headers don't render, scrollbar glyphs never appear, scroll-to-bottom doesn't happen). The 8 tests describe a coherent contract — the implementation has drifted from it. The module's failure rate is 8/64 = 12.5 %, an order of magnitude higher than the 1.2 % background rate, which strongly suggests an in-progress refactor rather than scattered drift.

**Recommended fix path:** either complete the refactor (make the cache lazy but observable, restore the sticky-header layout, add the scrollbar) or update the 8 tests to the new contract. **Do not** delete the tests — they encode the user-visible preview behavior.

**Confidence:** medium (real-bug) for the cluster; high for the cache-key one specifically.

### 2. Kitty keyboard protocol — `\x1b[>7u` reduced to `\x1b[>1u` — 4 tests, 1 root cause, all stale

Four tests in `crates/jcode-tui/src/tui/mod.rs` fail because the keyboard-enhancement flag value changed:

| Test | Got | Expected |
|------|-----|----------|
| `outside_tmux_keyboard_lifecycle_keeps_kitty_protocol_only` (L286) | `\x1b[>1u` | `\x1b[>7u` |
| `tmux_keyboard_lifecycle_requests_and_resets_extended_keys` (L271) | `\x1b[>4;2m\x1b[>1u` | `\x1b[>4;2m\x1b[>7u` |
| `focus_reapply_preserves_other_modes_without_rearming_focus_reporting` (L354) | `false` | `true` (looking for `\x1b[=7u`) |
| `keyboard_enhancement_flags_avoid_report_all_keys_escape_mode` (L2112) | `REPORT_EVENT_TYPES` absent | present |

**Root cause** (`crates/jcode-tui/src/tui/mod.rs:121-131`):
```rust
fn keyboard_enhancement_flags() -> crossterm::event::KeyboardEnhancementFlags {
    if std::env::var_os("KCODE_DISABLE_KITTY_KEYBOARD").is_some() {
        return KeyboardEnhancementFlags::empty();
    }
    // Only disambiguate escape codes. REPORT_EVENT_TYPES makes terminals emit
    // press AND release events; hosts that half-support the kitty protocol
    // (herdr/conpty panes) leak releases into the prompt as "[<code>;1:3u".
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES  // bit 1
}
```

This is an intentional, comment-justified narrowing of the protocol. `DISAMBIGUATE_ESCAPE_CODES` has bit value `1`; the test expectations encode the old `1|2|4 = 7` (all three reporting flags) form. The tests need to be updated to assert the new conservative value `1`. The behavior change itself is correct (the comment is accurate: `REPORT_EVENT_TYPES` causes release-event leaks in herdr/conpty panes — and the operator runs everything in herdr panes).

**Recommended fix path:** update the 4 test assertions to `\x1b[>1u` / `\x1b[=1u` and assert that `flags.contains(DISAMBIGUATE_ESCAPE_CODES)` but NOT `REPORT_EVENT_TYPES`. This is a 5-minute mechanical fix.

**Confidence:** high (stale).

### 3. Binary-rename cascade — `jcode` vs `kcode` — 7 tests, all stale

Seven tests assert the string `kcode` in command names, terminal-emitted notifications, or status-line text, but the production code still emits `jcode`:

| Test | Got | Expected | Source mismatch |
|------|-----|----------|-----------------|
| `build_resume_command_uses_imported_jcode_session_for_claude_code` (helpers_tests.rs:356) | `Some("kcode")` | `Some("jcode")` | helper vs test |
| `build_resume_command_uses_imported_jcode_session_for_codex` (helpers_tests.rs:383) | `Some("kcode")` | `Some("jcode")` | helper vs test |
| `kitty_notification_is_one_completed_clickable_message` (turn_notify.rs:543) | escape seq has `kcode-turn-…` | `jcode-turn-…` | notif vs test |
| `render_system_message_uses_connection_card_for_reconnect_status` (ui_messages/tests.rs:1947) | plain contains `jcode --resume` | `kcode --resume` | test vs itself (!) |
| `test_btw_command_forks_session_with_question` (commands_accounts_01:1175) | no `kcode --resume ` substring | substring present | production (commands_review.rs:768 emits `jcode --resume`) |
| `test_fork_command_with_prompt_forks_session` (1212) | same | same | same |
| `test_fork_command_without_prompt_forks_idle_session` (1248) | same | same | same |

**Root cause:** the KCode fork rebrand is in flight. Some call sites in `helpers.rs` / `turn_notify.rs` were already switched to `kcode`; production `commands_review.rs:768,775,824`, `commands.rs:460,467`, and `tui_lifecycle_runtime.rs:142` still emit `jcode`. Tests are split: 4 expect `kcode` (post-rebrand), 3 expect the `kcode` form in a string the production code emits as `jcode` (pre-rebrand inconsistency).

**Recommended fix path:** the canonical decision is recorded in `~/.jcode/memories/global/harness-agents.md` (the KCode decollision arc, 2026-10-07): **KCode reads jcode's data via a shim, but binary names do not auto-migrate** — the production command string is whatever the active binary name is. Either (a) emit `jcode --resume` everywhere for backwards compat (the historical user-visible string), or (b) emit `kcode --resume` everywhere and update the legacy sites. Until that decision is made, these 7 tests cannot be made green without a corresponding production edit.

**Note on test #4** (`render_system_message_uses_connection_card_for_reconnect_status`): the test's input string contains `jcode --resume koala` but the assertion on line 1947 checks for `kcode --resume koala`. **This test is internally inconsistent** — it can never pass without either the input being changed to `kcode` or the assertion being changed to `jcode`. The kcode-vs-jcode rename in the assertion is the only obvious explanation, but neither the input nor the source has the kcode form, so the test is broken in two ways.

**Confidence:** high (stale) for tests 1, 2, 3, 5, 6, 7. Medium for test 4 (self-inconsistent).

---

## Per-failure table (29 rows)

| # | Test | File:line | Class | Confidence | One-line diagnosis |
|--:|------|-----------|-------|------------|--------------------|
| 1  | `build_resume_command_uses_imported_jcode_session_for_claude_code` | helpers_tests.rs:356 | stale-expectation | high | helper now returns `kcode`; test asserts `jcode` |
| 2  | `build_resume_command_uses_imported_jcode_session_for_codex` | helpers_tests.rs:383 | stale-expectation | high | same as #1 |
| 3  | `detected_resume_terminal_recognizes_handterm_term_program` | helpers_tests.rs:205 | environment | high | HERDR env vars from wrapper shadow TERM_PROGRAM |
| 4  | `status_line_is_always_pinned_with_pink_model_on_real_app` | scroll_copy_01/part_01.rs:1356 | stale-expectation | medium | looks for pink model on last frame row; status bar (chunks[10]) now occupies that row |
| 5  | `test_btw_command_forks_session_with_question` | commands_accounts_01/part_01.rs:1175 | stale-expectation | high | splits on `kcode --resume `; production emits `jcode --resume` |
| 6  | `test_fork_command_with_prompt_forks_session` | commands_accounts_01/part_01.rs:1212 | stale-expectation | high | same |
| 7  | `test_fork_command_without_prompt_forks_idle_session` | commands_accounts_01/part_01.rs:1248 | stale-expectation | high | same |
| 8  | `test_handle_remote_disconnect_flushes_streaming_text_and_sets_reconnect_state` | remote_events_reload_02/part_01.rs:966 | stale-expectation | high | asserts `◈ Connection lost - retrying`; that string exists in no source file |
| 9  | `test_info_widget_local_direct_api_runtime_shows_cost_based_usage` | remote_events_reload_04.rs:1632 | stale-expectation | medium | asserts `cost.total_cost == 0.0` for KCODE_RUNTIME_PROVIDER=jcode; impl returns 0.075 (cost not suppressed) |
| 10 | `test_save_and_restore_reload_state_preserves_queued_messages` | remote_startup_input_03/part_01.rs:1039 | real-bug | medium | `restore_input_for_reload` returns `None` after `save_input_for_reload` — save path not writing |
| 11 | `test_tui_openai_compatible_empty_catalog_does_not_switch_to_profile_default` | state_model_poke_03.rs:1175 | real-bug | medium | empty catalog appears to activate a default provider model |
| 12 | `kitty_notification_is_one_completed_clickable_message` | turn_notify.rs:543 | stale-expectation | high | terminal escape emits `kcode-turn-…`; test expects `jcode-turn-…` |
| 13 | `preview_render_cache_is_reused_across_scroll_and_rebuilt_on_selection_change` | session_picker_tests.rs:2118 | real-bug | high | `picker.preview_cache` is `None` after first `render()` — eager cache build removed |
| 14 | `preview_visible_slice_matches_scroll_position` | session_picker_tests.rs:2173 | real-bug | medium | visible slice does not track scroll position |
| 15 | `search_highlights_match_in_preview_and_scrolls_to_it` | session_picker_tests.rs:2539 | real-bug | medium | search-scroll not auto-scrolling to match |
| 16 | `preview_without_search_has_no_highlight_and_scrolls_to_bottom` | session_picker_tests.rs:2585 | real-bug | medium | preview does not scroll to bottom by default |
| 17 | `test_preview_pane_shows_scrollbar_when_overflowing` | session_picker_tests.rs:1944 | real-bug | high | scrollbar glyph not rendered; raw "Test session" content shown instead |
| 18 | `test_preview_sticky_prompt_header_appears_after_scrolling` | session_picker_tests.rs:1998 | real-bug | high | sticky header row is empty (`row=""`) — header layout dropped |
| 19 | `test_preview_sticky_prompt_header_survives_async_preview_load` | session_picker_tests.rs:2066 | real-bug | medium | preview does not auto-scroll on async load (got 0) |
| 20 | `test_session_list_shows_scrollbar_when_overflowing` | session_picker_tests.rs:1967 | real-bug | high | list scrollbar glyph not rendered; raw content shown |
| 21 | `outside_tmux_keyboard_lifecycle_keeps_kitty_protocol_only` | mod.rs:286 | stale-expectation | high | impl now emits `\x1b[>1u`; test expects `\x1b[>7u` (intentional flag reduction) |
| 22 | `focus_reapply_preserves_other_modes_without_rearming_focus_reporting` | mod.rs:354 | stale-expectation | high | looks for `\x1b[=7u`; impl emits `\x1b[=1u` (same root cause as #21) |
| 23 | `tmux_keyboard_lifecycle_requests_and_resets_extended_keys` | mod.rs:271 | stale-expectation | high | same as #21 (tmux form: `\x1b[>4;2m\x1b[>1u` vs `\x1b[>4;2m\x1b[>7u`) |
| 24 | `cache_estimate_is_not_evidence_of_an_expected_warm_hit` | mod.rs:1885 | real-bug | high | `cache_expected_warm` ignores `is_estimate`; should return `false` when `is_estimate=true` |
| 25 | `keyboard_enhancement_flags_avoid_report_all_keys_escape_mode` | mod.rs:2112 | stale-expectation | high | same root cause as #21; `REPORT_EVENT_TYPES` no longer set |
| 26 | `render_system_message_uses_connection_card_for_reconnect_status` | ui_messages/tests.rs:1947 | stale-expectation | medium | self-inconsistent: input string has `jcode` but assertion looks for `kcode` |
| 27 | `overscroll_line_orders_dir_git_context_then_model_on_the_right` | swarm_buffer.rs:562 | stale-expectation | high | helper `overscroll_line_row` returns last row of frame; at width 200 the last row is now the status bar (chunks[10]), not the overscroll line (chunks[8]) |
| 28 | `overscroll_line_compacts_before_dropping_and_always_keeps_dir_model_context` | swarm_buffer.rs:591 | stale-expectation | high | same root cause as #27; at width 200 the row is status-bar content, not overscroll content |
| 29 | `overscroll_model_is_pink` | swarm_buffer.rs:636 | stale-expectation | high | same root cause as #27; looks for "GPT-5.6 Sol" on last row (status bar) |

---

## Cross-cutting observations

1. **The status-bar / layout shift** (rows 4, 27, 28, 29) is a single layout change with four test casualties. The new `status_bar::render_status_bar` at `chunks[10]` (line 3452 of `ui.rs`) means the "last row of the frame" no longer points at the overscroll line at any width ≥ 80. The 3 swarm_buffer tests + the 1 scroll_copy_01 test are the same defect. Fix: the test helpers `overscroll_line_row` (swarm_buffer.rs:546) and the `y = buf.area.height - 1` line in `overscroll_model_is_pink` (swarm_buffer.rs:629) need to either (a) target the overscroll chunk by computing the y-offset of `chunks[8]` from the layout, or (b) search rows for the directory/model substrings.

2. **The kitty-keyboard narrowing** (rows 21, 22, 23, 25) is one intentional change with four test casualties. The change is documented in the source comment (mod.rs:127-129) and is correct for the operator's herdr/conpty pane environment. The 4 tests need to be updated to the new conservative value `1` (DISAMBIGUATE_ESCAPE_CODES only) — this is a pure test edit, no production code change.

3. **The binary-rename cascade** (rows 1, 2, 5, 6, 7, 12, 26) is one KCode-decision with seven test casualties. The decollision arc memory (`~/.jcode/memories/global/harness-agents.md`) records that KCode reads jcode's data via a `home_dir()` shim, but **emitted user-visible strings are not auto-translated**. Until the rename reaches `commands_review.rs:768,775,824` and `commands.rs:460,467`, the 6 test-cases that split on `jcode --resume` will fail. The 7th case (row 26) is doubly broken — the test input has `jcode` but the assertion looks for `kcode`, and the rendering preserves the input.

4. **The session-picker cluster** (rows 13-20) is a single refactor. The implementation appears to have moved away from an eager-preview-cache model to a lazy/simplified model, but the sticky-header and scrollbar features described in the tests never made it across. The 8 tests are the contract; the implementation needs to satisfy it (or the tests need to be rewritten to describe the new contract — but that would be deleting user-visible features).

5. **Three lone real-bugs** worth flagging separately:
   - `cache_estimate_is_not_evidence_of_an_expected_warm_hit` (row 24) — clean 2-line fix in `crates/jcode-tui/src/tui/kv_cache_problem.rs:82-84`:
     ```rust
     // current (buggy):
     cache_ttl.map(|info| !info.is_cold).unwrap_or(false)
     // proposed:
     cache_ttl.map(|info| !info.is_cold && !info.is_estimate).unwrap_or(false)
     ```
   - `test_save_and_restore_reload_state_preserves_queued_messages` (row 10) — `save_input_for_reload` not persisting in this test path; needs deeper investigation (this test does not set `KCODE_HOME` to a temp dir like its siblings).
   - `test_tui_openai_compatible_empty_catalog_does_not_switch_to_profile_default` (row 11) — empty catalog activating a default model would be a user-visible behavior bug if reproducible in a real run.

---

## Reproducibility / re-run command

```bash
cd /Users/kooshapari/.jcode/scratch/wt-resume-relend
bash .audit/run-tui-tests.sh
# or, more directly:
cargo test -p jcode-tui --no-fail-fast 2>&1 | tee .audit/tui-test-output.log
grep FAILED$ .audit/tui-test-output.log > .audit/failures-raw.txt
# (29 lines expected; < 60 s incremental)
```

To reproduce a single failure for debugging:
```bash
cargo test -p jcode-tui <test-name> -- --nocapture
```

To dump all 26 captured panics in context:
```bash
grep -A 8 "panicked at" .audit/tui-test-output.log
```

To recover the 3 mystery panics from the `overscroll_*` tests (their panics are emitted with a race condition that sometimes loses them in the full-suite log):
```bash
cargo test -p jcode-tui --test '*' overscroll_line_orders_dir_git_context_then_model_on_the_right
cargo test -p jcode-tui --test '*' overscroll_line_compacts_before_dropping_and_always_keeps_dir_model_context
cargo test -p jcode-tui --test '*' overscroll_model_is_pink
```

---

## What's NOT a real bug — explicitly cleared

- **Helpers `KCODE_HOME` round-trip**: 6 of the 7 rename-affected tests use `let prev_home = std::env::var_os("KCODE_HOME"); ... set KCODE_HOME = temp; ... restore` correctly. The save/restore logic itself works (other tests in the same file that follow this pattern pass). Row 10 is a real bug, not a missing-restore-pattern bug.
- **Test parallelism / cargo nextest**: the run used the standard `cargo test` (not `cargo nextest`); all 29 failures reproduced on a single-process run, so this is not a parallelism artifact.
- **Build cache staleness**: the wrapper `cargo test` was run twice (background-fail + wrapper-success); both runs produced the same 29 failures. Not a stale-build artifact.

---

**Bottom line for the operator:** 19 of the 29 are stale tests that need updating (low cost — most are 1-line string changes); 9 are real bugs ranging from a 2-line `cache_expected_warm` fix to a 200+ line session-picker refactor restoration; 1 is an environment artifact of the test wrapper. The session-picker cluster (concern #1) and the kitty-keyboard narrowing (concern #2) are the highest-leverage triage targets — fixing them removes 12 of the 29 failures.
