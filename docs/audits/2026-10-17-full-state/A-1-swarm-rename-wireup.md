# A-1 Retry — Swarm Rename UI Wire-up (TUI)

**Branch:** `fix/a1-swarm-rename-wireup` (origin)
**Commit:** `e95ddb9d4` — `fix(tui/swarm-rename): wire TUI rename handler to real swarm dispatch`
**PR:** https://github.com/KooshaPari/KCode/pull/105
**Status:** OPEN (awaiting review)

## Bug (from A-1 audit lane)

The TUI swarm panel rename action in `crates/jcode-tui/src/tui/app/tui_state.rs:2326` showed a hard-coded `"Rename sent"` toast to the user but **silently dropped the new label**. The UI shell existed; the dispatch was never wired.

## Classification

A-1 (swarm rename UI lies) classified this as: **(c) feature unfinished** — UI present, dispatch stub never implemented.

## Diagnosis

- `Request::RenameSession` had no `target_session_id` field
- The TUI never populated `target_session_id` in the outgoing request
- Status message was hard-coded `"Rename sent"` regardless of whether the request was sent

## Fix (commit `e95ddb9d4`)

| Crate | Change |
|-------|--------|
| `jcode-protocol` | Added `target_session_id: Option<String>` to `Request::RenameSession` |
| `jcode-app-core` (server) | `client_lifecycle` dispatch now handles `target_session_id` (follows `SetReasoningEffort` pattern) |
| `jcode-app-core` | `rename_session()` now accepts optional `target_session_id` |
| `jcode-tui` | `RenameConfirm` populates `pending_swarm_rename`; remote key handler drains it and dispatches the request with target session id |

## Tests added

- 2 new protocol roundtrip tests for `target_session_id` in `crates/jcode-protocol/src/protocol_tests/core_events.rs`

## Validation

- `cargo check --workspace` (per `tx-validated: cargo-check` trailer)
- 11 files changed, +140/-26 LOC

## Author

`Koosha Paridehpour <42529354+KooshaPari@users.noreply.github.com>`

## Ledger trailers

```
tx-agent:     jcode
tx-validated: cargo-check
tx-scope:     crates/jcode-tui, crates/jcode-protocol, crates/jcode-app-core
tx-intent:    TUI swarm rename now actually renames the target member session
```
