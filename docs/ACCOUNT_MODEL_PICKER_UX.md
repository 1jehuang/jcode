# Account and model picker UX

## Scope and interpretation

The request was interpreted as improving the existing terminal account center and inline account/model pickers: clear action scope, safe account actions, visible results, and stable model selection. The terminal target was inferred from this repository and its AGENTS.md, which places Jcode Desktop in a separate repository. The user did not explicitly choose between those interfaces or prescribe a visual style.

The initial UX commit was rechecked against this interpretation. Its production changes all serve those interactions. Provider availability and session-pin metadata fixes were necessary follow-through for failures observed through the pickers. No earlier production change needed reverting after the scope became clear. Test-environment isolation did not change credential routing, and model-context precedence was left untouched.

## Account management

- `/accounts` is the sole account slash command. It opens the full account center, focused on this window's saved login, with add, rename, reauthentication, removal, switching, and per-account usage. Explicit account searches retain the inline quick-switch picker. Claude Code instances are included in quick switching when configured or when the current window uses Claude Code.
- Enter uses a saved account in this window. Ctrl+D makes a saved Claude/OpenAI login the default for new and unpinned windows, from either the command preview or the focused picker. A pinned window keeps its account. Ctrl+D on another action or an empty result preserves the query and explains that a saved login must be selected.
- Plain letters, including `d`, `j`, `k`, `g`, and `q`, search instead of triggering account actions or closing the account center. Arrow keys navigate. In the account center, Left/Right jump provider groups and Home/End and PageUp/PageDown navigate the list.
- Add-login and refresh-login descriptions distinguish creating another saved login from refreshing a particular saved login. Provider settings are not duplicated in the account center.
- Removing an account from the account center opens a confirmation showing the exact target. Only lowercase `y` with no modifiers confirms. Enter does not delete. Escape, navigation, search, or a mouse event cancels confirmation.
- Removal deletes saved Jcode credentials, not the external provider account. Direct `/accounts <provider> remove <label>` commands retain their existing behavior.
- Compact terminals use a full-width account center. Provider headers, scrolling, and mouse hit testing share the same physical-row calculation so the selected action remains visible.
- The inline account picker reserves space for its hint even when search leaves just one account or no matches. Hints describe the selected action, and an empty search result shows how to clear the filter. Enter with no matches keeps the account or model preview open without discarding the query. Generic slash-command suggestions cannot overwrite an open picker's rows.
- Per-window native login pins remain visible when the active model uses a direct compatible API runtime. Account-change events, catalog responses, and history agree with the saved session pin. A live runtime's resolved pin or default still wins during failover and unpinning. This does not change the compatible endpoint's API credentials.
- Standalone sessions use the same metadata policy. Compatible API routes do not display a native OpenAI login badge just because their provider name starts with OpenAI.

## Model selection

- Opening the model picker focuses the current model. Without a current model, it prefers an available saved default, then an available route.
- The current model remains visible even if its route is temporarily unavailable.
- Clearing search with Escape or broadening it with Backspace retains the selected model identity while it still matches, including when the final character is removed. Existing provider-scope route restoration still applies when clearing an `@provider` scope.
- Enter on an unavailable model keeps the picker and search open and explains why the route is unavailable. It does not stage a switch. An unavailable route cannot be saved as the default with Ctrl+O.
- Escape from the reasoning-level chooser restores the model picker, including its selected route, search, and navigation column.
- Interaction tests use explicitly available routes and verify that Enter stages a route selection, rather than only checking that the picker closes.
- An active named provider profile determines availability only for the generic OpenAI-compatible endpoint. Dedicated provider credentials and local-provider enablement remain independent.

## Validation

Initial account/model validation was run locally on macOS on October 13, 2026. Those isolated runs did not change the running Jcode daemon or installed launcher. The later test-failure fix and requested installation are separate follow-up steps.

```bash
JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-app-core --lib session_accounts -- --test-threads=1

JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-base --lib provider_catalog -- --test-threads=1

JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-tui-account-picker --lib

JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-tui --lib -- \
  account model_browser model_search focused_browser bare_model \
  model_with_name initial_model_selection preview_enter_after_arrow \
  unavailable_model effort_step inline_picker ctrl_d --test-threads=1

cargo fmt --all -- --check
git diff --check
```

| Test run | Passed |
| --- | ---: |
| Account-picker crate | 13 |
| Focused TUI and Ctrl+D | 119 |
| Provider catalog | 43 |
| Server/session accounts | 17 |
| Total | 192 |

Account checks cover rendering, compact viewport and mouse mapping, removal confirmation, window/default scope, search, and local/remote shortcuts. Model checks cover unavailable routes, identity preservation, and reasoning-step navigation. Catalog and socket tests cover provider isolation and consistent persisted-pin metadata in events, catalog responses, and history.

Before the follow-up isolation fix, the complete TUI suite passed 2,865 tests, with 2 failures and 18 ignored tests. Both failures also reproduced in a complete run of the original commit `0b4ce4365`. The original run had 8 failures. At the first UX commit, the refined run had 6 failures. The provider-isolation fix and generic account fixture isolation eliminated four of those failures.

Four new provider-isolation tests failed before the production fix and passed afterward. They check both directions: an unconfigured named profile cannot hide configured built-in providers, and a configured named profile cannot enable unconfigured built-ins. Failing-first checks also reproduced disappearing account rows, discarded empty-preview queries, Ctrl+D editing the preview instead of acting on the picker, selection jumps while broadening a shared-prefix model search, and persisted-pin metadata disagreement. The corresponding regression tests now pass.

The two context-widget tests passed in isolation but failed in the earlier complete suites:

- `test_info_widget_remote_model_falls_back_to_model_provider_detection`
- `test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context`

The cause was confirmed by running just the auth-doctor fixture before these two tests: the same test executable changed from two passing widget tests to two failures with a cached 200,000 limit instead of the expected 1,000,000. The auth-doctor fixture initialized ambient configuration before temporary-home app fixtures, seeding custom GPT context limits into the process-global cache.

Commit `213189d57` fixes the fixture isolation without changing production context-limit precedence or weakening the widget assertions. Both tests now use a temporary Jcode home and a test-support guard that snapshots, clears, and restores the context cache, including during panic unwinding. A failing-first regression verifies isolation and preservation of custom GPT limits. A second regression verifies restoration after panic.

After the fix, the exact auth-doctor/widget sequence passed all three tests, the context/catalog run passed 54 tests, and the complete TUI suite passed **2,867 tests with zero failures and 18 ignored**. Logs are `context-contaminated-repro.log`, `context-guard-red.log`, `context-guard-green.log`, `context-contaminated-fixed.log`, `fixed-base-context-catalog.log`, and `fixed-full-tui.log` under `$JCODE_SCRATCH_DIR/account-model-ux`.

Builds emitted nonfatal local Rust toolchain warnings about a missing `libLLVM.dylib` during debug stripping and a large linker unwind section. Test results above are from executed test binaries, not inferred from compiler success.

## Requirement audit and observed outcomes

After the refinements, the whole-result audit reran every mapped check, the complete TUI suite, and all 11 real-terminal checks. The final code was tested at clean commit `b58dfb58c` with a fresh `v0.95.257-dev` CLI. The table distinguishes terminal/socket observations from regression assertions. It does not claim every edge case was exercised against a live provider.

| ID | Requirement and observed result | Executed evidence |
| --- | --- | --- |
| A1 | Opening accounts selected this window's account. Configured and implicit current Claude Code instances were included. | Terminal checks 1/3; `test_account_ux_overview_focuses_current_claude_code_instance` and implicit-instance regression |
| A2 | Enter pinned Fox without changing the Otter default. Ctrl+D made Fox default without changing this window's Otter pin. Other socket sessions stayed unchanged. | Terminal checks 3/5; `set_session_account_does_not_change_other_sessions_or_default`; preview/default regressions |
| A3 | Plain navigation/default letters searched. Empty Enter/Ctrl+D retained query and picker. Unsupported default actions displayed guidance rather than editing the filter or executing. | Terminal check 4; `search_letters_and_empty_enter_never_execute`; `account_preview_ctrl_d_without_saved_account_preserves_query`; local/remote interaction regressions |
| A4 | One OpenAI settings result emitted the settings command. Add copy said existing accounts are kept. Claude/OpenAI refresh actions rendered the exact saved target and masked email, never the raw email, and emitted the matching targeted command. | `test_account_ux_center_does_not_duplicate_provider_settings`; `test_account_ux_add_and_refresh_render_exact_target_and_masked_identity`; compact-title and hint regressions. Commands were inspected, not dispatched into OAuth. |
| A5 | Public account help aliases described window-only selection, Ctrl+D, kept accounts, and saved-credential removal with confirmation. The old bare-`d` instruction was absent. | `account_help_matches_window_default_and_removal_safety` |
| A6 | Single-result and no-match rows stayed visible alongside hints. Late generic slash completion no longer erased account rows. | Terminal checks 1/2/4; full-frame single/empty-result regressions; `account_picker_rows_survive_late_command_overlay` |
| A7 | Compact selection stayed visible. At tested list heights 1 through 8, every selected physical row rendered and mouse clicks selected the corresponding action. Background outside the overlay remained intact. | Terminal check 6 at 50×14; `physical_rows_keep_every_selection_visible_and_map_mouse_exactly`; mouse/background regressions |
| A8 | Removal showed the exact target. Enter did not delete, Escape canceled, and explicit unmodified `y` removed only the synthetic target. Navigation, filtering, and mouse events disarmed both confirmation forms. | Terminal checks 6/7; `removal_requires_y_and_changes_disarm_for_both_command_forms` |
| M1 | The current model opened selected, even when blocked in the regression fixture. With no current row, an available default or fallback won. Bare Enter opened the browser rather than switching silently. | Terminal check 8; `initial_model_selection_prefers_current_then_available_default_then_available`; bare-model regression |
| M2 | Escape and successive Backspaces retained model, provider, and API-method identity through shared-prefix search. Empty preview Enter kept its query. | Terminal checks 9/10; identity/route, scope-clear and empty-preview regressions |
| M3 | Enter on an unavailable route kept the picker/filter and explained the problem. No switch was staged, and Ctrl+O could not save the unavailable default. | `unavailable_model_enter_keeps_search_and_explains_why`; `unavailable_model_cannot_be_saved_as_default` |
| M4 | Escape from reasoning selection restored the parent model route, filter, and column. Chosen effort applied locally or staged remotely. Rendered labels identified the model and levels. | `effort_step_esc_returns_to_the_model_list_with_filter_kept`; local/remote effort-selection and `model_effort_step_names_the_model_and_lists_levels` regressions |
| M5 | Selecting the available route changed the real server to ux-beta. Reopening marked ux-beta current. Interaction fixtures also asserted a pending route, not just picker dismissal. | Terminal check 11; `focused_browser_typing_searches_and_enter_selects`; `preview_enter_after_arrow_navigation_selects_that_row` |
| P1 | An unconfigured named generic profile did not hide dedicated Comtegra/FPT credentials or enabled Ollama. A configured generic profile did not enable unconfigured built-ins. | All four named-profile availability regressions and the complete 43-test catalog run |
| S1 | Direct-runtime account-change events, catalog, and history reported Fox pinned rather than Otter unpinned. Unpin followed the default. Live failover/resolution remained authoritative over the stored pin. Existing resume/stream checks passed. | Real socket `direct_runtime_persisted_pin_is_reported_in_account_event_catalog_and_history`; metadata regressions and the complete 17-test session-account run; terminal checks 3/5 |
| S2 | Standalone sessions retained persisted pins when the runtime could not report them. Compatible API routes did not claim a native OpenAI OAuth account badge. | `standalone_account_metadata_uses_persisted_pin_without_runtime_report`; `compatible_api_does_not_claim_native_openai_account_family`; terminal footer captures |
| C1 | Generic slash hints still rendered without a picker. Ctrl+D outside account selection still edited text or requested quit, locally and remotely. Model preview/focused rendering checks stayed green. | `generic_slash_suggestions_remain_visible_without_picker`; local/remote Ctrl+D editor tests; all inline-picker render tests |

The audit initially found a coverage gap in A4: the older settings test checked absence of a duplicate title but not the surviving command, and targeted refresh identity was not asserted. The final tests close that gap through rendered frames and emitted actions. A verifier required every mapped test to appear as executed and passing, checked the terminal binary/source provenance and clean process exits, and allowed only the two named baseline failures in the full TUI run. Its per-requirement results and test-executable hashes are in `$JCODE_SCRATCH_DIR/account-model-ux/audit-map-results.json` and `audit-test-provenance.txt`.

Earlier failing-first or terminal runs observed erased account rows, discarded empty-preview queries, Ctrl+D editing a query instead of using account controls, Fox/Otter metadata disagreement, and Backspace jumping from ux-beta to ux-alpha. The final runs passed those same interactions. That is the evidence for improved behavior, not a claim of user preference or a human usability study.

## Real-terminal acceptance

A fresh `target/debug/jcode` served an isolated socket and a real PTY client. The fixture used temporary HOME/JCODE_HOME directories, synthetic saved logins, a loopback model catalog, disabled telemetry and updates, and a rejecting loopback proxy for external requests. No real account credentials were changed. Input was typed through the terminal, rendered output was captured, and read-only debug observations checked the selected rows and applied server state.

All 11 acceptance checks passed across 80×24, 50×14, and 110×32 terminal sizes:

1. The current account and window/default guidance are visible, including a single filtered account.
2. Enter pins Fox in this window while the saved default remains Otter.
3. Plain `j`, `k`, `d`, and `q` search. Enter and Ctrl+D on an empty result preserve the picker and query.
4. Ctrl+D makes Fox the saved default while the window stays pinned to Otter. Reopening confirms the two scopes remain distinct.
5. The compact account center keeps the selected removal visible. Enter does not confirm deletion, Escape cancels, and only explicit `y` removes the exact synthetic account.
6. The model picker opens on the current model. Escape and successive Backspaces clear a shared-prefix search without changing model or route identity.
7. Selecting the available loopback model reaches the server, and reopening selects the new current model.

The local harness and artifacts are under `$JCODE_SCRATCH_DIR/account-model-ux`. Each `pty-*` run records `result.json` with binary SHA-256, version metadata, source commit, checks, and process exit codes. Successful runs include numbered screen captures and `run/terminal.ansi`. The owned client and server both exited cleanly.

The initial whole-result rerun is `pty-6daf27mh/result.json`, with source commit `b58dfb58c` and binary SHA-256 `5d915ede412ac931239dd8e35aa33079f233b5f900e8b2885cd9c7164b387329`. The later context-cache change is test support and test fixtures only, not production context resolution.

This initial terminal run did not exercise real provider OAuth or token refresh. Unavailable-route handling and reasoning-column restoration are covered by regressions, not by the all-available loopback catalog. That run left the installed launcher and shared daemon unchanged.
