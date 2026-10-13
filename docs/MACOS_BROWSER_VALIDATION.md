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
