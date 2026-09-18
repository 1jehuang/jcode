# [04] Remote session picker and slash commands over SSH

## Requirement

Over SSH (`jcode --ssh`), most slash commands were dead: the remote input
path either blocked them outright or executed them against the *laptop's*
local state instead of the remote session. User-visible symptoms: `/save`,
`/todos`, `/tool-call-details`, `/cancel`, `/catchup`, `/back`, `/git`,
`/restart`, `/resume`, `/sessions` and the display-preference commands did
nothing or touched the wrong machine. `/resume` specifically was blocked
(`ssh_local_action_blocked`) because the session picker reads session files
from local storage — useless when the sessions live on the server.

This patch adds a small wire surface and a remote command dispatch so both
the session picker and the common slash commands operate on the remote
daemon's state.

## Wire protocol (`jcode-protocol`)

- `Request::ListSessions { id }` → `ServerEvent::SessionList { id, sessions }`:
  one `SessionListEntry` per daemon-owned session (title, counts, timestamps,
  working dir, model/provider, status, saved flags, live-attachment flags).
- `Request::SessionPreview { id, session_id }` →
  `ServerEvent::SessionPreviewResult { id, session_id, messages }`: a bounded
  preview (last ~20 rendered messages: role, text, tool names). When the list
  lands the picker prefetches previews for every session in list order (a few
  in flight at a time, selected row first), so paging through rows renders
  from cache — no per-row round trips.
- `Request::SetSessionSaved { id, session_id, saved, save_label }` — the
  server loads the session file, toggles the saved flag, persists it.
  `/save` and `/unsave` round-trip over SSH.
- `Request::GetTodos { id, session_id }` → `ServerEvent::Todos { id, todos,
  goals, plan }` (uses `jcode-task-types`, new protocol dep). `/todos`
  fetches server-side todo state and renders the inline card.
- All of these are registered in `Request::id()` and
  `is_lightweight_control_request`, so they answer even mid-turn and on a
  connection that has not subscribed to a session yet; dispatched in
  `client_lifecycle`/`client_lightweight_control`, implemented in
  `session_listing.rs`.

## Session picker over SSH

Selecting a daemon-owned session queues `resume_session`, which the existing
remote drain turns into `Request::ResumeSession` — resume switches the
current TUI's session on the server, in this terminal, with no full-SSH TUI
needed.

Semantics preserved:

- Sessions sorted by last activity descending; empty sessions and
  `imported_*` stems excluded (the local picker groups imports under their
  own sources).
- Title precedence: `custom_title` → todo-derived session title → generated
  title → short name — same as the local summary pass.
- `needs_catchup` stays client-side: the TUI computes it from its local
  `CatchupSeenSnapshot` (seen-state is per-client).
- Enter (any terminal mode) and multi-select resume the first daemon-owned
  target in this terminal; external transcripts (Claude Code/Codex/Pi/etc.)
  remain unavailable remotely — importing them would read the wrong
  machine's storage — and show the standard SSH blocked notice.

## Slash commands over SSH

Client (`jcode-tui`):

- `backend.rs`: `RemoteConnection::list_sessions` / `session_preview` /
  `set_session_saved` / `get_todos`.
- The remote blocklist now only blocks genuinely-local commands
  (accounts/billing, server file edits, laptop-DB stats, laptop file
  opens, selfdev/ssh, sims). `/save` `/unsave` `/todo` `/todos` pass.
- `dispatch_ssh_local_command` handles client-rendered commands:
  `/cancel` `/help` `/diff` `/cls`, display prefs (alignment, reasoning,
  thinking-display, compact-notifications, show-agentgrep-output,
  tool-call-details), keys/dictation/colors/feedback/telemetry/debug/info.
- Remote skill invocation resolves names against `remote_skills`; the
  server expands the body via `active_skill`.
- `key_handling` remote chain: `/cancel` `/stop` send
  `remote.cancel_with_reason` (fixes a pre-existing dead flag); `/restart`
  re-execs preserving `JCODE_SSH_REMOTE`; `/zstatus` shows remote premium
  mode; `/z` `/zz` `/zzz` skip laptop config writes under SSH; `/catchup`
  `/catchup next` `/back` go through the remote session list + return
  stack; `/git` runs server-side `git status` via `input_shell`;
  improve/refactor metadata persistence is gated behind `is_ssh_remote`.
- Pickers: `open_active_sessions_picker` and `open_catchup_picker` get
  remote-list flows (loading overlay + `pending_remote_session_list`,
  live presence + catchup filters); catchup-mode selection goes through
  `queue_catchup_resume` so `/back` works.
- `apply_remote_session_list` stashes `remote_catchup_candidates` and
  resolves pending `/catchup next` fetches; the remote tick drains
  `pending_remote_todos_request`.

Decision: display-preference commands persist to the *laptop* config —
they configure this client's rendering of remote output and are
intentionally client-scoped.

## Server side (`crates/jcode-app-core/src/server/session_listing.rs`)

- `handle_list_sessions`: snapshots live state (`SessionAgents` keys →
  `live_attached`; `client_connections` `is_processing` → `live_processing`),
  then scans `~/.jcode/sessions/*.json` on the blocking pool. Each entry is a
  full `Session::load` (journal replay included) — correct and simple; the
  count of sessions on a personal daemon is small. Skips non-`.json` files,
  `.pre-wipe-*` backups, and `imported_*` stems.
- `handle_session_preview`: `Session::load` + `session::render_messages` (the
  same renderer the local preview uses), last 20 rows.
- `handle_set_session_saved`: loads the session file, toggles the saved
  flag, persists.
- `handle_get_todos`: reads server-side todo state for the session.

## TUI picker internals (`crates/jcode-tui`)

- `commands_dispatch.rs`: `/resume`/`/sessions`/bare `/session` removed from
  `ssh_unsupported_command`; a dedicated handler opens the picker in the SSH
  dispatch branch before the generic local-fallback block.
- `open_session_picker`: SSH path builds a `SessionPicker::loading()` with
  `remote_preview_mode` and queues `pending_remote_session_list`; the remote
  tick loop sends `list_sessions` and drains the picker's prefetch queue into
  `session_preview` requests (a few in flight at a time). The resume
  keybinding in `remote/key_handling.rs` calls `open_session_picker` too —
  the SSH branch there means no separate remote path is needed.
- `handle_session_picker_key` no longer blocks SSH mode up front: keys flow
  into the picker normally now that the picker itself is remote-capable.
- `session_picker.rs`: `remote_preview_mode` keeps a `remote_preview_queue`
  + `remote_preview_inflight` window instead of a local loader thread. When
  the session list lands, `queue_remote_previews` seeds every listed session
  so arrow-key paging renders from cache rather than waiting on a per-row
  round trip; `ensure_selected_preview_loading` moves the selected row to
  the front of the queue. Live presence is seeded from the wire
  `live_attached`/`live_processing` flags (`set_remote_live_presence`) so the
  working/ready badges and the Active filter work without the local pid
  registry; `refresh_live_presence` re-applies that snapshot in remote mode.
  The crashed-session restore banner is suppressed in remote mode — recovery
  is a local-filesystem operation — while rows keep their Crashed badge.
- `loading.rs`: `session_info_from_wire` maps `SessionListEntry` →
  `SessionInfo` (search index built from metadata only; preview content joins
  the index when it arrives, same as local).
- `inline_interactive.rs`: `apply_remote_session_list` wraps entries into a
  single `ServerGroup` named after the SSH host, reseeds the open picker,
  then seeds live presence + the preview prefetch queue;
  `handle_remote_session_picker_selection` resumes daemon sessions via the
  existing `workspace_client.queue_resume_session` drain.

## Still blocked (no server feature exists)

`/fix`, `/usage`, `/config` `/permissions` `/agents` `/swarm-prompt`,
`/overnight` `/initiatives` `/goals`, `/hosted` `/subscribe` `/account`
`/accounts` `/logout` `/auth`, `/stats` `/productivity` `/wrapped`
`/model-status` `/provider-test-coverage` `/cache`, `/open` `/file`
`/new-terminal` `/transcript`, `/selfdev` `/ssh` `/remote` `/log`
`/support`, `/update-sim` `/onboarding-sim` `/onboarding-preview`.
Wiring these needs new server-side request handlers — follow-up work.

## Limitations (v1)

- Live presence is a snapshot from when the list was fetched (it does not
  track remote attach/detach after the fact; reopen the picker to refresh).
- Preview tool data (`tool_data`) is not sent over the wire; tool names are.
- Full `Session::load` per listed session — fine at personal-daemon scale; if
  listing ever becomes hot, move the streaming summary deserializer from
  `jcode-tui/session_picker/loading.rs` into a shared crate.

## Affected files

- `crates/jcode-protocol/src/wire.rs`, `src/lib.rs`, `Cargo.toml` — new
  request/event variants + `SessionListEntry`/`SessionPreviewMessage` +
  `jcode-task-types` dep + chrono; `Cargo.lock`.
- `crates/jcode-app-core/src/server.rs`, `server/client_lifecycle.rs`,
  `server/client_lightweight_control.rs`, `server/session_listing.rs` —
  module + dispatch arms + handlers.
- `crates/jcode-tui/src/tui/backend.rs` — remote RPC methods.
- `crates/jcode-tui/src/tui/session_picker.rs`, `session_picker/loading.rs` —
  remote preview mode + wire mapper.
- `crates/jcode-tui/src/tui/app.rs`, `app/inline_interactive.rs`,
  `app/tui_lifecycle.rs`, `app/commands.rs`, `app/commands_dispatch.rs`,
  `app/auth_remote/command.rs`, `app/helpers/model_names.rs`,
  `app/remote.rs`, `app/remote/key_handling.rs`,
  `app/remote/input_dispatch.rs`, `app/remote/server_events.rs`,
  `app/remote/session_persistence.rs`, `app/todos_view.rs` — state, drains,
  key handling, event arms, command routing, unblock + tests.
- `crates/jcode-tui/src/tui/app/tests/ssh_remote.rs` — updated tests.

## Upstream candidacy

Yes — general features (remote session picker + remote command coverage
over the existing request/event protocol), not local config. The design
mirrors the local picker's loading semantics closely enough to be a
reasonable PR candidate; the wire variants are additive and safe to PR.
