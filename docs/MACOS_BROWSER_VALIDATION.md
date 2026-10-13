# macOS browser validation

The macOS browser tool has two backends: Browser Agent Bridge for Brave, Chrome, Opera, and Firefox, and native AppleScript for Safari. These checks exercise the compiled tool directly, without a browser-agent model credential or an existing Jcode daemon.

## Prerequisites

Run `jcode browser setup <browser>` for each browser. Complete the normal extension approval in each extension browser. If setup repairs the unpacked Chromium extension's Opera detector, reload Browser Agent Bridge in every browser using that directory.

For Safari, approve macOS Automation access and enable Allow JavaScript from Apple Events. macOS may require the account password in its own authorization dialog. No password belongs in chat, a shell command, or a test fixture. Open a normal webpage so status can verify permission with a constant string probe.

All requested browsers may remain running. Do not close other browsers to make a mismatched target pass.

## Validation record

The coordinator's machine at 09:47:59 UTC on October 13, 2026 was an arm64 Mac running macOS 26.6.2 (build 25G83), rustc 1.99.0 (`b940084d7`, September 28, 2026), and cargo 1.99.0 (`5f94df478`, August 27, 2026). Available tools included `/usr/sbin/lsof`, `/usr/bin/osascript`, and `/opt/homebrew/bin/node`. This acceptance evidence covers one physical Apple Silicon Mac, not an Intel Mac or universal browser compatibility.

The final frozen-source aggregate validation completed successfully at 10:30:07 UTC on October 13, 2026 (job `337645nz1w`, exit 0). All five actual core workflows passed: Brave, Chrome, Opera, Firefox, and Safari. The same job passed 88 base browser tests and 89 app-core browser tests, with 7 ignored, and rebuilt the CLI after the Safari fix. Each live workflow checked exact browser identity, fresh page reading, form typing and selection, JavaScript, click navigation, and the rendered destination receipt.

Additional targeted checks included four fresh-receipt Opera repetitions and Firefox and Safari runs with conflicting inherited browser, WebSocket, and session settings. Safari's post-fix run passed in 7.08 seconds and closed its owned window. All 16 Safari tests passed, including script syntax and stable-ID generation checks. The creation fix replaces lazy ordinal window references with numeric native IDs while preserving the exact bootstrap nonce, layout, and ownership guards.

Earlier failures are not discarded: one repetition timed out before tests ran because a child inherited a Cargo gate descriptor. The child-spawn fix prevents that descriptor from escaping while retaining the build gate in its owner. An observed Firefox identity probe exceeded its original 500 ms process deadline under load. The process probe now has a bounded 5-second budget, and socket-owner checks inspect the target PID rather than trusting inherited routing or an unrelated process. PID, user, socket, and browser-parent proof remain required. The earlier Opera failure was not proven to have the same cause.

Cleanup is ownership-limited. If an exact fixture identity cannot be found or verified, leave the tab intact rather than closing a possibly unrelated user page.

## Native bridge scope policy

The five workflows above passed with explicit tab IDs. They do not verify upstream macOS session isolation. On October 13, 2026, an isolated installed-client probe supplied a live synthetic PID and Unix socket through `BROWSER_SESSION`, with `FAB_WS_URL=ws://127.0.0.1:0`. The installed CLI reported `browser 1.0.0`, while its installation marker was `v0.10.1`. Both `ping {}` and a separate harmless `getContent {}` probe accepted no socket connection and failed a direct WebSocket connection with OS error 49. The page-read probe had a live listener before command launch and did not contact a real browser. In the exact upstream v0.10.1 source, `bound_window_id` is startup output only, not session state or outgoing target injection.

Jcode's macOS native-bridge policy requires a concrete positive `tab_id` for page reads and mutations. Window-only scope is insufficient for legacy Firefox. Known creation and metadata commands `newSession`, `newWindow`, `listTabs`, and `getActiveTab` may run without page scope. Composite and unknown raw actions, and any `params.fork`, are rejected even with an outer ID. File upload is disabled because the tagged legacy CLI rebuilds upload parameters without `tabId`, even when an outer ID is supplied. Screenshots are also denied, including explicitly targeted requests, because the installed `captureVisibleTab` fallback can race the active tab. At outgoing serialization, `newSession` always receives `returnContent=false`, including raw caller `true`; its result is creation metadata and content requires a separate explicit-tab read. Null parameters normalize to an empty object. Nonobject parameters and creator fields outside the scalar creation allowlist are rejected before dispatch. Metadata commands accept only an empty object or valid `tabId` and `windowId` fields, never nested or unknown options. The native path skips managed bridge sessions and clears inherited `BROWSER_SESSION`. Safari retains its separate owned-window policy. Non-macOS session behavior is unchanged.

The follow-up regressions passed on October 13, 2026 through `scripts/dev_cargo.sh` with the host-wide gate and `CARGO_TARGET_DIR=$HOME/.jcode/scratch/jcode-macos-browser-target`. Serialization tests cover forced content suppression, null normalization, malformed and nested creators, strict metadata shape, capture denial, and an unchanged explicit-tab read. The public-tool test uses only a synthetic live PID and socket, and checks that unscoped requests fail before readiness or dispatch without accessing a real page.

The approved `live_macos_bridge_explicit_tab_ignores_selected_decoy` runs passed separately for Chrome and Firefox with `--ignored --nocapture`. Each run creates two fresh loopback fixture tabs and verifies their returned IDs, URL and nonce before interaction. Raw creation explicitly requests `returnContent=true` and checks that no implicit content is returned. With the owned decoy selected, read, type and eval address the first tab explicitly. The first-tab receipt is distinct, its input and dataset change, and the decoy snapshot must remain unchanged. Fixture tabs are retained with exact IDs printed. No existing user page, screenshot, upload, close command, credentials or service is used. The two runs are recorded below. They establish the tested explicit-tab behavior, not general upstream session isolation.

### Follow-up execution record

The production patch received independent read-only review with no remaining HIGH or MEDIUM findings. A later test-only correction received root review. The two task results are separate, not one successful aggregate.

- Original aggregate `536862p3ea`, October 13, 2026, 11:38:56 to 11:40:46 UTC, exited **101**. Focused macOS regressions passed 10 tests with 1 ignored. App-core browser tests passed 96 with 8 ignored. Base browser tests passed 88. The CLI build completed. All five existing explicit-tab workflows passed one actual test each under hostile inherited `FAB_BROWSER`, `FAB_WS_URL` and `BROWSER_SESSION`, with auto-update disabled. Chrome's new decoy harness then failed at its first identity evaluation, which returned null because the script lacked an explicit `return`. No decoy mutation was reached, and Firefox's decoy test was not run in this aggregate.
- Affected rerun `8223409k9x`, October 13, 2026, 11:43:42 to 11:44:25 UTC, exited **0**. Only test scripts and known-owned logging changed. Focused regressions again passed 10 with 1 ignored, app-core browser tests again passed 96 with 8 ignored, and the fresh CLI build completed. Chrome and Firefox each passed one real owned-decoy test. Production was unchanged, so the passed base suite and five-browser matrix were not repeated.

Both rounds used the app-core test executable `debug/deps/jcode_app_core-22afdb72a4decbf5` in the coordinated target. Each round compiled its current source before live execution. The affected rerun retained the hostile inherited environment and disabled auto-update.

| Original workflow | Exact owned tab | Receipt |
| --- | --- | --- |
| Brave | 1376646115 | macos-browser-1791891619189240000 |
| Chrome | 2131965038 | macos-browser-1791891623303472000 |
| Opera | 1060303086 | macos-browser-1791891627719397000 |
| Firefox | 15 | macos-browser-1791891634556052000 |
| Safari | 32517 | macos-browser-1791891637427360000 |

Safari's exact owned fixture window was closed by the existing guarded workflow. The extension-browser fixture tabs were retained.

| Affected decoy proof | First tab | Decoy tab | Nonce | Loopback endpoint |
| --- | --- | --- | --- | --- |
| Chrome | 2131965040 | 2131965041 | 1791891854338868000 | 127.0.0.1:49906 |
| Firefox | 16 | 17 | 1791891859569549000 | 127.0.0.1:49996 |

Both creations explicitly requested `returnContent=true` and returned no implicit content. The exact owned URL and nonce were verified before typing or document mutation. With the owned decoy selected, the explicit first-tab read returned its distinct receipt, typing and evaluation changed the first tab, and the decoy snapshot remained unchanged. These exact fixture tabs were left open. The failed preliminary Chrome fixture was also left untouched.

This follow-up was validated on one macOS arm64 machine with its installed integrations. It does not certify Intel macOS or runtime behavior on non-macOS platforms. Safari's separate owned-window adapter and non-macOS dispatch policy were unchanged. No browser installation, reload, signing change, shared-daemon replacement, or master merge was performed.

Nonfatal build warnings included the local toolchain's missing `libLLVM.dylib` during debug stripping, linker unwind-table size, unmatched profile package entries and the macOS-unused non-macOS fallback helper. They did not prevent successful test or build completion.

## Regression tests

From your isolated checkout root, use a shared target directory only after acquiring the coordinated build slot. Replace the checkout placeholder with your own path:

```bash
cd /path/to/jcode-checkout
export CARGO_TARGET_DIR="$HOME/.jcode/scratch/jcode-macos-browser-target"
cargo test -p jcode-base --no-default-features browser --lib
cargo test -p jcode-app-core --no-default-features tool::browser --lib
cargo build --bin jcode
```

Coverage includes native-host registration repair, exact browser identity, explicit child environment routing, native-host discovery despite a changed temporary directory, Opera's detector, bounded Unix socket names, Safari ownership across sessions, nonprompting status, consent-only setup, subprocess timeouts, document identity, fail-closed cleanup, unsupported actions, and password annotation redaction.

## Actual browser workflows

The ignored test is opt-in because it creates real browser tabs and requires installed, approved integrations:

```bash
for browser in brave chrome opera firefox safari; do
  JCODE_BROWSER_AUTO_UPDATE=0 \
  JCODE_BROWSER_LIVE_BROWSER="$browser" \
    cargo test -p jcode-app-core --no-default-features \
      live_macos_browser_core_workflow --lib -- --ignored --nocapture || exit 1
done
```

Each run:

1. Requires readiness for the exact requested browser.
2. Serves a disposable page on an ephemeral loopback port, with a fresh receipt.
3. Creates a tab and checks that page reading returns that receipt.
4. Types into a form, selects an option, and verifies the actual DOM values.
5. Clicks the page button, waits for the rendered destination, and checks its receipt and URL query fields.
6. Closes the owned Safari window through the provider's guarded cleanup.

Extension-browser tests print their disposable tab and window IDs for cleanup. Close only those test-created tabs and windows, not existing user pages. A failed Safari identity check deliberately refuses cleanup rather than closing a potentially repurposed user window.

The signed Firefox bridge 0.9.7 supports the normal-tab API used by this fixture, but not the optional `newWindow` request. A pass through `new_tab` does not establish support for that unsupported raw option.

Repeat Chrome with Opera active. Also repeat with conflicting inherited `FAB_BROWSER`, `FAB_WS_URL`, and `BROWSER_SESSION` values to verify that an explicit target is not redirected by stale settings.

## Scope

A core workflow pass verifies actual reading, form interaction, JavaScript, clicking, navigation, and rendered output. It does not certify every website or optional action. Safari uses synthetic DOM input in dedicated session-owned windows, supports only the top frame, and explicitly rejects screenshots, frame targeting, and file upload. Same-URL reloads or delayed redirects can invalidate Safari's document handle, requiring a new dedicated tab.

The optional model-based `handoff` workflow has its own provider authentication requirement. Successful direct browser tests do not establish that those credentials are available.
