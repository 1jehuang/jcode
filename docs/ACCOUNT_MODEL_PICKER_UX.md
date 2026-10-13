# Account and model picker UX

## Account management

- `/account` focuses the account this window uses. Claude Code instances are included when configured or when the current window uses Claude Code.
- Enter uses a saved account in this window. Ctrl+D makes a saved Claude/OpenAI login the default for new and unpinned windows, from either the command preview or the focused picker. A pinned window keeps its account. Ctrl+D on another action or an empty result preserves the query and explains that a saved login must be selected.
- Plain letters, including `d`, `j`, `k`, `g`, and `q`, search instead of triggering account actions or closing the account center. Arrow keys navigate. In the account center, Left/Right jump provider groups and Home/End and PageUp/PageDown navigate the list.
- Add-login and refresh-login descriptions distinguish creating another saved login from refreshing a particular saved login. Provider settings are not duplicated in the account center.
- Removing an account from the account center opens a confirmation showing the exact target. Only lowercase `y` with no modifiers confirms. Enter does not delete. Escape, navigation, search, or a mouse event cancels confirmation.
- Removal deletes saved Jcode credentials, not the external provider account. Direct `/account <provider> remove <label>` commands retain their existing behavior.
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

Validation was run locally on macOS on October 13, 2026. The running Jcode daemon and installed launcher were not changed.

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
| Focused TUI and Ctrl+D | 117 |
| Provider catalog | 43 |
| Server/session accounts | 17 |
| Total | 190 |

Account checks cover rendering, compact viewport and mouse mapping, removal confirmation, window/default scope, search, and local/remote shortcuts. Model checks cover unavailable routes, identity preservation, and reasoning-step navigation. Catalog and socket tests cover provider isolation and consistent persisted-pin metadata in events, catalog responses, and history.

The final complete TUI suite passed 2,863 tests, with 2 failures and 18 ignored tests. Both remaining failures also reproduced in a complete run of the original commit `0b4ce4365`. The original run had 8 failures. At the first UX commit, the refined run had 6 failures. The provider-isolation fix and generic account fixture isolation eliminated four of those failures.

Four new provider-isolation tests failed before the production fix and passed afterward. They check both directions: an unconfigured named profile cannot hide configured built-in providers, and a configured named profile cannot enable unconfigured built-ins. Failing-first checks also reproduced disappearing account rows, discarded empty-preview queries, Ctrl+D editing the preview instead of acting on the picker, selection jumps while broadening a shared-prefix model search, and persisted-pin metadata disagreement. The corresponding regression tests now pass.

The remaining tests passed when rerun together in an isolated test process, but failed in both complete suites:

- `test_info_widget_remote_model_falls_back_to_model_provider_detection`
- `test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context`

Their context lookup uses a process-global model context cache. The full-suite-only behavior is consistent with cache contamination. This change leaves model-context precedence untouched and does not claim those existing suite failures are fixed.

Builds emitted nonfatal local Rust toolchain warnings about a missing `libLLVM.dylib` during debug stripping and a large linker unwind section. Test results above are from executed test binaries, not inferred from compiler success.

## Real-terminal acceptance

A fresh `target/debug/jcode` served an isolated socket and a real PTY client. The fixture used temporary HOME/JCODE_HOME directories, synthetic saved logins, a loopback model catalog, disabled telemetry and updates, and a rejecting loopback proxy for external requests. No real account credentials were changed. Input was typed through the terminal, rendered output was captured, and read-only debug observations checked the selected rows and applied server state.

All 11 acceptance checks passed across 80×24, 50×14, and 110×32 terminal sizes:

1. The current account and window/default guidance are visible, including a single filtered account.
2. Enter pins Fox in this window while the saved default remains Otter.
3. Plain `j`, `k`, `d`, and `q` search. Enter on an empty result preserves the picker and query.
4. Ctrl+D makes Fox the saved default while the window stays pinned to Otter. Reopening confirms the two scopes remain distinct.
5. The compact account center keeps the selected removal visible. Enter does not confirm deletion, Escape cancels, and only explicit `y` removes the exact synthetic account.
6. The model picker opens on the current model. Escape and successive Backspaces clear a shared-prefix search without changing model or route identity.
7. Selecting the available loopback model reaches the server, and reopening selects the new current model.

The local harness and artifacts are under `$JCODE_SCRATCH_DIR/account-model-ux`. Each `pty-*` run records `result.json` with binary SHA-256, version metadata, source commit, checks, and process exit codes. Successful runs include numbered screen captures and `run/terminal.ansi`. The owned client and server both exited cleanly.

This terminal run did not exercise real provider OAuth or token refresh. Unavailable-route handling and reasoning-column restoration are covered by regressions, not by the all-available loopback catalog. The installed launcher and shared daemon were left unchanged.
