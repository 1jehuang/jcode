# Account and model picker UX

## Account management

- `/account` focuses the account this window uses. Claude Code instances are included when configured or when the current window uses Claude Code.
- Enter uses a saved account in this window. Ctrl+D makes a saved Claude/OpenAI login the default for new and unpinned windows. A pinned window keeps its account.
- Plain letters, including `d`, `j`, `k`, `g`, and `q`, search instead of triggering account actions or closing the account center. Arrow keys navigate. In the account center, Left/Right jump provider groups and Home/End and PageUp/PageDown navigate the list.
- Add-login and refresh-login descriptions distinguish creating another saved login from refreshing a particular saved login. Provider settings are not duplicated in the account center.
- Removing an account from the account center opens a confirmation showing the exact target. Only lowercase `y` with no modifiers confirms. Enter does not delete. Escape, navigation, search, or a mouse event cancels confirmation.
- Removal deletes saved Jcode credentials, not the external provider account. Direct `/account <provider> remove <label>` commands retain their existing behavior.
- Compact terminals use a full-width account center. Provider headers, scrolling, and mouse hit testing share the same physical-row calculation so the selected action remains visible.
- The inline account picker reserves space for its hint even when search leaves just one account or no matches. Hints describe the selected action, and an empty search result shows how to clear the filter.

## Model selection

- Opening the model picker focuses the current model. Without a current model, it prefers an available saved default, then an available route.
- The current model remains visible even if its route is temporarily unavailable.
- Clearing search with Escape or deleting its last character retains the selected model identity. Existing provider-scope route restoration still applies when clearing an `@provider` scope.
- Enter on an unavailable model keeps the picker and search open and explains why the route is unavailable. It does not stage a switch. An unavailable route cannot be saved as the default with Ctrl+O.
- Escape from the reasoning-level chooser restores the model picker, including its selected route, search, and navigation column.
- Interaction tests use explicitly available routes and verify that Enter stages a route selection, rather than only checking that the picker closes.
- An active named provider profile determines availability only for the generic OpenAI-compatible endpoint. Dedicated provider credentials and local-provider enablement remain independent.

## Validation

Validation was run locally on macOS on October 13, 2026. The running Jcode daemon and installed launcher were not changed.

```bash
JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-base --lib provider_catalog -- --test-threads=1

JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-tui-account-picker --lib

JCODE_TELEMETRY=off JCODE_NO_TELEMETRY=1 DO_NOT_TRACK=1 \
  cargo test -p jcode-tui --lib -- \
  account model_browser model_search focused_browser bare_model \
  model_with_name initial_model_selection preview_enter_after_arrow \
  unavailable_model effort_step --test-threads=1

cargo fmt --all -- --check
git diff --check
```

The account-picker crate passed all 13 tests, the focused TUI run passed all 88 tests, and the provider catalog passed all 43 tests. These 144 checks cover actual full-frame account rendering, small-terminal viewport and mouse mapping, explicit removal confirmation and cancellation, account/default scope, search keys and empty results, unavailable routes, model identity preservation, reasoning-step navigation, and named/built-in provider isolation.

The final complete TUI suite passed 2,854 tests, with 2 failures and 18 ignored tests. Both remaining failures also reproduced in a complete run of the original commit `0b4ce4365`. The original run had 8 failures. At the first UX commit, the refined run had 6 failures. The provider-isolation fix and generic account fixture isolation eliminated four of those failures.

Four new provider-isolation tests failed before the production fix and passed afterward. They check both directions: an unconfigured named profile cannot hide configured built-in providers, and a configured named profile cannot enable unconfigured built-ins. The empty-account-search full-frame test also failed with the old row sizing and passed with the new sizing.

The remaining tests passed when rerun together in an isolated test process, but failed in both complete suites:

- `test_info_widget_remote_model_falls_back_to_model_provider_detection`
- `test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context`

Their context lookup uses a process-global model context cache. The full-suite-only behavior is consistent with cache contamination. This change leaves model-context precedence untouched and does not claim those existing suite failures are fixed.

Builds emitted nonfatal local Rust toolchain warnings about a missing `libLLVM.dylib` during debug stripping and a large linker unwind section. Test results above are from executed test binaries, not inferred from compiler success.
