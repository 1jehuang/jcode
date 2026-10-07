# macOS browser session reuse

Two independent bugs caused the window buildup and delay:

1. The external `1jehuang/firefox-agent-bridge` CLI checked `/proc/<pid>` in
   `rust-cli/src/commands/session.rs::is_session_running`. macOS has no `/proc`.
   This affected `session list`, duplicate-start detection, and routing commands
   through `BROWSER_SESSION`.
2. Jcode's `crates/jcode-base/src/browser_session.rs` used Jcode's runtime
   directory. The bridge uses `$XDG_RUNTIME_DIR`, falling back to `/tmp`, and
   ignores both `$TMPDIR` and `$JCODE_RUNTIME_DIR`. On macOS, Jcode waited ten
   seconds for a socket in the wrong directory, killed the successful daemon,
   and retried without `--bind-window` for another ten seconds. The first
   process had already created a Chrome window.

Jcode now follows the bridge's socket directory and verifies both the PID and
an accepting socket. Startup is serialized, and a failed bound start no longer
launches an unbound fallback. The bridge companion change replaces `/proc` with
`kill(pid, 0)` (including `EPERM`), verifies its listener, saves bound-window
metadata, reuses that window after a crash, and closes it on stop or SIGTERM.
Stopping uses the session socket rather than signalling an unverified stale PID.
Failed window cleanup preserves metadata so the next start can reuse the window.

The bridge is a separate repository, not a vendored Jcode crate. Its companion
change is based on `b8b0c14` and includes the extension's background `closeWindow`
action; it does not change extension UI or Jev handoff logic. Both CLI and
extension changes must ship for automatic window cleanup. Persistent sessions
remain unsupported on Windows; direct commands are unchanged.

## Verification on Apple Silicon macOS

Before the fix, a live session responded to `ping` in 17 ms while `session list`
reported it dead. Regression tests failed on the live PID and runtime path.

After the fix, a harness using rebuilt Jcode session code and the rebuilt bridge
CLI against the installed Chrome extension made five `listTabs` calls. There was
one new Chrome window, one daemon, and identical tab/window IDs throughout.
Times were 113, 6, 7, 5, and 6 ms. `session list` reported `running`.
The diagnostic daemon and window were removed afterward.

The installed 0.10.0 extension lacks `closeWindow`, so automatic window cleanup
was verified with a simulated native host and tests of the extension's actual
cleanup function. Live cleanup with the updated extension remains a rollout
check. The lifecycle test covers five calls, crash recovery, graceful stop,
SIGTERM, stopping a crashed session, replacing a manually closed window, and
refusing to signal an unrelated process from a stale PID file.

```sh
# Jcode: run on both macOS and Linux in the existing CI matrix.
cargo test -p jcode-base --lib browser -- --nocapture
cargo test -p jcode-app-core --lib browser_session -- --nocapture

# Bridge repository: a new CI matrix runs these on macOS and Linux.
cargo test --manifest-path rust-cli/Cargo.toml --locked
node --test scripts/test-session-window.js
```

Local validation passed on macOS. Linux validation is configured in CI and was
not executed locally.

The broader Jcode checks have pre-existing failures: the size, panic, and
swallowed-error budgets report unrelated files, and Rust 1.99 Clippy rejects
`out.push_str("…")` in `tool/mcp.rs:49`. Clippy for both affected crates passes
with that existing `single_char_add_str` lint allowed. Changed Rust files pass
formatting, and module resolution passes.
