# [08] Backport: stop re-arming focus reporting from focus events

## Requirement

Backport of upstream `38f7967a1` ("fix: avoid rearming focus reports from
focus events", fixes upstream #1184), which landed after the base tag this
branch tracks.

## Bug

`FocusGained` handlers re-asserted terminal modes, including DECSET 1004
(focus reporting). Ghostty (and some other terminals) answer a 1004
enable with their current focus state — i.e. enabling focus reporting can
itself produce a `FocusGained`. That feeds the handler's output back into
its input forever: a focus-event ping-pong loop.

Over SSH this manifests as a frozen remote view: the remote event loop's
biased `select!` keeps servicing the terminal-event stream (which never
goes idle because of the ping-pong), starving `next_event` from the remote
connection. The spinner still animates locally but remote session events
stop rendering.

## Fix (verbatim upstream port)

- `reapply_configured_terminal_modes` →
  `reapply_configured_terminal_modes_after_focus`, which reasserts mouse
  capture and Kitty keyboard enhancement but passes `focus_change =
  false`, leaving mode 1004 alone.
- Startup and resume-after-editor paths still enable focus reporting
  normally — only the FocusGained re-assert skips it.
- Call sites: `local.rs` `apply_terminal_event`, `remote.rs`
  `apply_terminal_event` and `handle_terminal_event_while_disconnected`,
  plus the fork's extra `FocusGained` handler in remote session-picker
  handling that upstream did not yet have at the commit.
- Unit test `focus_reapply_preserves_other_modes_without_rearming_focus_reporting`
  ported with it.

## Affected files

- `crates/jcode-tui/src/tui/mod.rs` — renamed fn + `focus_change = false`
  + test.
- `crates/jcode-tui/src/tui/app/local.rs`, `app/remote.rs` — call sites.

## Upstream status

This IS upstream code — drop this patch when the branch rebases onto a
tag that contains `38f7967a1`.
