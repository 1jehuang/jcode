# Client-Core vs Presentation Split Plan

Status: Proposed

This document audits the current TUI/client stack and proposes a safe, incremental split between a reusable `client-core` layer and the ratatui/crossterm presentation layer.

The goal is to make the current single-surface client easier to maintain, while also unblocking the multi-surface direction described in [`MULTI_SESSION_CLIENT_ARCHITECTURE.md`](./MULTI_SESSION_CLIENT_ARCHITECTURE.md).

See also:

- [`REFACTORING.md`](./REFACTORING.md)
- [`MULTI_SESSION_CLIENT_ARCHITECTURE.md`](./MULTI_SESSION_CLIENT_ARCHITECTURE.md)
- [`SERVER_ARCHITECTURE.md`](./SERVER_ARCHITECTURE.md)

## Executive Summary

Today the client stack is functionally split, but not structurally split:

- `src/tui/app.rs` owns a very large `App` state object with session state, transport state, input state, transient UI state, and runtime handles mixed together.
- `src/tui/app/*.rs` acts like a distributed reducer, but mutation is expressed as direct `impl App` methods instead of typed actions and reducer entrypoints.
- `src/tui/ui.rs` and `src/tui/ui_*.rs` are already mostly presentation-only, but they still depend on a very wide `TuiState` trait and a few process-global render caches.
- `src/tui/workspace_client.rs` is process-global mutable state, which is the clearest current blocker for a true client-core split and for multi-surface clients.

The safest plan is:

1. Define a real `client-core` state model inside the existing crate first.
2. Move pure state and reducers behind that boundary without changing behavior.
3. Keep ratatui rendering, overlays, markdown, mermaid, and render caches in presentation.
4. Only after the boundary is clean, consider moving `client-core` into its own crate.

## Current Stack Audit

## Entry points and loops

Current runtime entrypoints:

- `src/cli/tui_launch.rs`
  - boots terminal runtime
  - constructs `tui::App`
  - restores session/startup hints
  - calls `app.run(...)`
- `src/tui/app/run_shell.rs`
  - local loop: `App::run`
  - remote loop: `App::run_remote`
  - replay loop helpers
- `src/tui/app/local.rs`
  - local tick handling
  - terminal event handling
  - bus event handling
  - finish-turn bookkeeping
- `src/tui/app/remote.rs`
  - remote tick and terminal event handling
  - reconnect and disconnected handling
- `src/tui/app/remote/reconnect.rs`
  - connect/reconnect orchestration
- `src/tui/app/remote/input_dispatch.rs`
  - remote send/split submission path
- `src/tui/app/remote/server_events.rs`
  - main remote event reducer today

Rendering entrypoints:

- `src/tui/mod.rs`
  - `render_frame(frame, state)`
- `src/tui/ui.rs`
  - `draw(frame, app: &dyn TuiState)`
  - `draw_inner(...)`
- `src/tui/ui_prepare.rs`, `ui_viewport.rs`, `ui_messages.rs`, `ui_input.rs`, `ui_pinned.rs`, `ui_overlays.rs`, `ui_header.rs`, `ui_diagram_pane.rs`
  - frame preparation and rendering

## Current state root

Primary root:

- `src/tui/app.rs`
  - `pub struct App`
  - `DisplayMessage`
  - `ProcessingStatus`
  - several transport and pending-operation helper structs

`App` currently mixes all of these concerns:

- runtime handles
  - `provider`, `registry`, `skills`, `mcp_manager`, debug channel
- conversation/session data
  - `messages`, `session`, `display_messages`, tool-output tracking
- composer/input state
  - `input`, `cursor_pos`, pasted content, pending images, queueing
- turn execution state
  - `is_processing`, `status`, `processing_started`, pending turn flags
- streaming state
  - `streaming_text`, stream buffer, thinking state, token usage, TPS tracking
- remote client/session state
  - remote provider hints, session ids, server metadata, reconnect/startup state, split launch state
- workspace state
  - currently not in `App`, but in global `workspace_client.rs`
- surface-local UI state
  - scroll offsets, copy selection, diagram pane focus/scroll, diff pane state, inline picker state, overlays, status notices
- config and feature toggles
  - memory, swarm, diff mode, centered mode, diagram mode, auto-review, auto-judge

## Current mutation surface

Mutation is spread across many `impl App` files:

State helpers and pseudo-reducers:

- `src/tui/app/state_ui.rs`
- `src/tui/app/state_ui_runtime.rs`
- `src/tui/app/state_ui_messages.rs`
- `src/tui/app/state_ui_storage.rs`
- `src/tui/app/state_ui_input_helpers.rs`
- `src/tui/app/state_ui_maintenance.rs`
- `src/tui/app/conversation_state.rs`

Event and command handling:

- `src/tui/app/input.rs`
- `src/tui/app/turn.rs`
- `src/tui/app/local.rs`
- `src/tui/app/remote.rs`
- `src/tui/app/remote/input_dispatch.rs`
- `src/tui/app/remote/server_events.rs`
- `src/tui/app/remote/workspace.rs`
- `src/tui/app/navigation.rs`
- `src/tui/app/inline_interactive.rs`
- `src/tui/app/copy_selection.rs`
- `src/tui/app/model_context.rs`
- `src/tui/app/auth*.rs`

This is why the code already feels reducer-like, but is still tightly coupled. State transitions, runtime side effects, and redraw decisions are interleaved.

## Current presentation boundary

The renderer already has a partial boundary via `src/tui/mod.rs::TuiState`.

That boundary is promising, but still too wide because it currently includes:

- raw domain/session access
- surface state access
- auth/config lookups
- render-specific helpers such as `render_streaming_markdown`
- some expensive derived computations and caching behavior
- mutable behavior like `update_cost`

The result is that the trait is acting as a dump point rather than a narrow presentation model.

## Concrete pain points found in code

### 1. `App` is too large and semantically mixed

The state root in `src/tui/app.rs` is carrying:

- domain state
- surface/controller state
- transport state
- runtime handles
- presentation-adjacent state

This prevents reuse outside the current TUI runtime.

### 2. No typed action/reducer boundary

The main reducers are implicit:

- `local.rs::handle_tick`
- `local.rs::handle_terminal_event`
- `remote/server_events.rs::handle_server_event`
- `remote/input_dispatch.rs::*`
- `state_ui_messages.rs::*`
- `conversation_state.rs::*`

These should become named reducers over named state slices.

### 3. Workspace state is process-global

- `src/tui/workspace_client.rs`
  - uses `static WORKSPACE_STATE: Mutex<Option<WorkspaceClientState>>`

This is incompatible with:

- multiple client instances in one process
- test isolation without global resets
- future multi-surface clients
- a clean client-core extraction

This state must become instance-owned.

### 4. Render layer still relies on globals

Examples in `src/tui/ui.rs`:

- `LAST_MAX_SCROLL`
- `PINNED_PANE_TOTAL_LINES`
- prompt viewport animation state
- visible copy targets

These are presentation concerns, but they should become renderer-instance state, not process-global state.

### 5. Runtime loops and rendering are tightly interwoven

`terminal.draw(|frame| crate::tui::ui::draw(frame, &self))` appears in many control-flow paths:

- `run_shell.rs`
- `turn.rs`
- `remote/reconnect.rs`
- `input.rs`
- `model_context.rs`

That makes controller extraction harder because redraw timing is coupled to mutation paths.

## Proposed Split

## Layer 1: `client-core`

Owns client behavior and state, but not ratatui rendering or terminal I/O.

Allowed in core:

- client/session/surface state
- reduction of user intents, server events, bus events, and ticks
- command parsing and routing decisions
- queueing and pending-operation state
- workspace model state
- feature toggles and mode state
- effects emitted for runtime adapters

Not allowed in core:

- `ratatui`
- `crossterm` event types
- direct terminal drawing
- process-global UI caches
- widget rendering
- mermaid/image/markdown rendering details

## Layer 2: presentation

Owns all ratatui and render-time concerns.

Includes:

- `src/tui/ui.rs` and `src/tui/ui_*.rs`
- `src/tui/info_widget*.rs`
- `src/tui/markdown*.rs`
- `src/tui/mermaid*.rs`
- `src/tui/session_picker*.rs`
- `src/tui/login_picker.rs`
- `src/tui/account_picker*.rs`
- `src/tui/usage_overlay.rs`
- `src/tui/visual_debug.rs`

Presentation should consume a narrow immutable snapshot or read-only trait from core.

## Proposed State Types

These types should exist before any crate split. Initially they can live in a new `src/client_core/` module inside the main crate.

### `ClientCoreState`

Top-level state for one client surface.

Suggested file:

- `src/client_core/state/mod.rs`

Suggested fields:

- `conversation: ConversationState`
- `composer: ComposerState`
- `turn: TurnState`
- `stream: StreamState`
- `remote: RemoteState`
- `workspace: WorkspaceState`
- `surface: SurfaceState`
- `features: FeatureState`
- `notices: NoticeState`

### `ConversationState`

Suggested files:

- `src/client_core/state/conversation.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/conversation_state.rs`
- `src/tui/app/state_ui_messages.rs`

Owns:

- `messages: Vec<Message>`
- `display_messages: Vec<DisplayMessage>`
- `display_messages_version: u64`
- tool output tracking
  - `tool_call_ids`
  - `tool_result_ids`
  - `tool_output_scan_index`
- provider/session conversation hydration helpers

Reducer name:

- `conversation_reducer`

Primary responsibilities:

- append/replace/remove display messages
- replace provider transcript
- compact storage-friendly display messages
- maintain tool output tracking

### `ComposerState`

Suggested file:

- `src/client_core/state/composer.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/state_ui_input_helpers.rs`
- pure parts of `src/tui/app/input.rs`

Owns:

- `input`
- `cursor_pos`
- `pasted_contents`
- `pending_images`
- `queued_messages`
- `hidden_queued_system_messages`
- `interleave_message`
- `pending_soft_interrupts`
- `pending_soft_interrupt_requests`
- `stashed_input`
- `queue_mode`
- `submit_input_on_startup`
- route-next-prompt flags

Reducer names:

- `composer_reducer`
- `queue_reducer`

Primary responsibilities:

- text editing
- queueing/interleave behavior
- restore/save reload input decisions
- turning prepared input into a high-level send intent

### `TurnState`

Suggested file:

- `src/client_core/state/turn.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/local.rs`
- `src/tui/app/tui_lifecycle.rs`
- `src/tui/app/state_ui_maintenance.rs`

Owns:

- `is_processing`
- `status: ProcessingStatus`
- `processing_started`
- `pending_turn`
- `pending_queued_dispatch`
- `cancel_requested`
- `quit_pending`
- `pending_provider_failover`
- `session_save_pending`
- background maintenance state
- current-turn reminder state

Reducer names:

- `turn_reducer`
- `lifecycle_reducer`
- `maintenance_reducer`

Primary responsibilities:

- start/finish turn
- idle/sending/streaming/tool transitions
- failover countdown state
- maintenance banners/notices

### `StreamState`

Suggested file:

- `src/client_core/state/stream.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/remote/server_events.rs`
- `src/tui/app/misc_ui.rs`

Owns:

- `streaming_text`
- `stream_buffer`
- `streaming_tool_calls`
- token usage fields
- cache usage fields
- TPS tracking fields
- thinking/thought-line state
- `last_stream_activity`
- `subagent_status`
- `batch_progress`

Reducer names:

- `stream_reducer`
- `server_event_reducer`

Primary responsibilities:

- text delta/replace handling
- tool start/exec/done state
- token accounting
- thought-line handling
- stale activity tracking

### `RemoteState`

Suggested file:

- `src/client_core/state/remote.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/remote/input_dispatch.rs`
- `src/tui/app/remote/server_events.rs`
- `src/tui/app/remote/reconnect.rs`
- `src/tui/app/remote/queue_recovery.rs`

Owns:

- remote session identity and resume state
- provider/model/server metadata
- startup/reconnect phase
- split-launch state
- pending remote message state
- rate-limit retry state
- remote resume activity snapshot
- `current_message_id`
- server sessions / client count / swarm snapshots

Reducer names:

- `remote_reducer`
- `server_event_reducer`
- `remote_lifecycle_reducer`

Primary responsibilities:

- reduce `ServerEvent` into remote/session state
- own remote reconnect-visible state
- own split/new-session routing state
- own queue recovery bookkeeping

### `WorkspaceState`

Suggested file:

- `src/client_core/state/workspace.rs`

Move in from:

- `src/tui/workspace_client.rs`

Owns:

- `enabled`
- `map: WorkspaceMapModel`
- `imported_server_sessions`
- `pending_split_target`
- `pending_resume_session`

Reducer names:

- `workspace_reducer`

Primary responsibilities:

- enable/disable workspace mode
- import existing sessions
- update map after split/resume/history sync
- move focus left/right/up/down

Important rule:

- this state must become instance-owned, not global static state

### `SurfaceState`

Suggested file:

- `src/client_core/state/surface.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/navigation.rs`
- `src/tui/app/copy_selection.rs`
- `src/tui/app/inline_interactive.rs`
- selected non-render code from `src/tui/app/input.rs`

Owns:

- `scroll_offset`
- `auto_scroll_paused`
- copy selection state
- diff pane focus/scroll
- diagram focus/index/scroll/ratio state
- side-panel focus state
- inline interactive/view state
- help/changelog overlay visibility and scroll
- status notices
- mouse scroll animation queue

Reducer names:

- `surface_reducer`
- `navigation_reducer`
- `overlay_reducer`

Note:

This is still core, not presentation. It is surface-local controller state, not render cache state.

### `FeatureState`

Suggested file:

- `src/client_core/state/features.rs`

Move in from:

- `src/tui/app.rs`
- `src/tui/app/observe.rs`
- `src/tui/app/split_view.rs`

Owns:

- memory, swarm, autoreview, autojudge, improve mode
- diff mode
- centered mode
- diagram mode/pinning defaults
- observe mode
- split view mode
- image pinning and native scrollbar toggles

Reducer names:

- `feature_reducer`

### `NoticeState`

Suggested file:

- `src/client_core/state/notices.rs`

Owns:

- transient status notices
- rate-limit/reset countdown notices
- background task wake/status notices
- startup hints / restored reload notices

Reducer names:

- `notice_reducer`

## Proposed Effects Boundary

Reducers should not directly call terminal, remote socket, or persistence APIs.

Introduce:

- `src/client_core/effects.rs`

Suggested effect enum:

- `ClientEffect::SendRemoteMessage { ... }`
- `ClientEffect::ResumeRemoteSession { session_id }`
- `ClientEffect::LaunchRemoteSplit`
- `ClientEffect::PersistSession`
- `ClientEffect::PersistReloadInput`
- `ClientEffect::ExtractMemories`
- `ClientEffect::StartCompaction`
- `ClientEffect::RunInputShell { ... }`
- `ClientEffect::RequestQuit`
- `ClientEffect::RequestRedraw`

Runtime adapters in `src/tui/app/local.rs`, `remote.rs`, `remote/reconnect.rs`, and `run_shell.rs` should execute these effects.

## Presentation: What Stays Put

The following should remain presentation-owned for the first split:

### Core renderer

- `src/tui/ui.rs`
- `src/tui/ui_prepare.rs`
- `src/tui/ui_viewport.rs`
- `src/tui/ui_messages.rs`
- `src/tui/ui_input.rs`
- `src/tui/ui_pinned.rs`
- `src/tui/ui_overlays.rs`
- `src/tui/ui_header.rs`
- `src/tui/ui_diagram_pane.rs`
- `src/tui/ui_layout.rs`
- `src/tui/ui_status.rs`
- `src/tui/ui_theme.rs`

### Rendering helpers and caches

- `src/tui/markdown*.rs`
- `src/tui/mermaid*.rs`
- `src/tui/image.rs`
- `src/tui/visual_debug.rs`
- render cache structs in `ui.rs`, `ui_messages_cache.rs`, `ui_file_diff.rs`, `ui_pinned.rs`

### Widgets and overlays

- `src/tui/info_widget*.rs`
- `src/tui/session_picker*.rs`
- `src/tui/login_picker.rs`
- `src/tui/account_picker*.rs`
- `src/tui/usage_overlay.rs`

## Concrete File Mapping

### Files that should become core-first

| Current file | Target module | Notes |
| --- | --- | --- |
| `src/tui/app.rs` | `src/client_core/state/*` + thin `App` shell | Split the giant `App` root by concern |
| `src/tui/app/conversation_state.rs` | `src/client_core/state/conversation.rs` | Mostly state logic already |
| `src/tui/app/state_ui_messages.rs` | `src/client_core/reducer/conversation.rs` | Clean first reducer extraction |
| `src/tui/app/state_ui_input_helpers.rs` | `src/client_core/reducer/composer.rs` | Pure text-edit logic |
| `src/tui/app/state_ui.rs` | `src/client_core/reducer/lifecycle.rs` | Save/restore and client focus helpers |
| `src/tui/app/state_ui_maintenance.rs` | `src/client_core/reducer/maintenance.rs` | Notice/message state |
| `src/tui/app/remote/server_events.rs` | `src/client_core/reducer/server_event.rs` | Highest-value reducer split |
| `src/tui/app/remote/queue_recovery.rs` | `src/client_core/reducer/queue_recovery.rs` | Already isolated |
| `src/tui/app/remote/workspace.rs` | `src/client_core/reducer/workspace.rs` + runtime adapter | Split commands from transport calls |
| `src/tui/workspace_client.rs` | `src/client_core/state/workspace.rs` | Must stop being global |
| `src/tui/app/navigation.rs` | `src/client_core/reducer/navigation.rs` | Move non-ratatui navigation state |
| `src/tui/app/copy_selection.rs` | `src/client_core/reducer/copy_selection.rs` | Surface interaction state |
| `src/tui/app/inline_interactive.rs` | `src/client_core/reducer/inline_ui.rs` | State transitions, not drawing |

### Files that should remain presentation-first

| Current file | Keep in presentation because... |
| --- | --- |
| `src/tui/ui.rs` | main ratatui frame renderer |
| `src/tui/ui_prepare.rs` | render-time wrapping/caching/layout prep |
| `src/tui/ui_viewport.rs` | draw-time viewport calculations |
| `src/tui/ui_messages.rs` | ratatui message rendering |
| `src/tui/ui_input.rs` | input box drawing |
| `src/tui/ui_pinned.rs` | side-pane drawing and caches |
| `src/tui/info_widget*.rs` | widget composition and rendering |
| `src/tui/markdown*.rs` | rendering pipeline, not client behavior |
| `src/tui/mermaid*.rs` | rendering pipeline and image management |
| `src/tui/session_picker*.rs`, `login_picker.rs`, `account_picker*.rs`, `usage_overlay.rs` | widget state can remain presentation initially |

## Recommended Reducer API

Do not start with a single mega-reducer.

Start with slice reducers and one coordinator:

- `reduce_tick(state, now) -> Effects`
- `reduce_terminal_intent(state, intent) -> Effects`
- `reduce_server_event(state, event) -> Effects`
- `reduce_bus_event(state, event) -> Effects`
- `reduce_workspace_action(state, action) -> Effects`

Suggested types:

- `ClientIntent`
  - normalized user intent, not raw crossterm keys
- `ExternalEvent`
  - server event, bus event, tick, lifecycle event
- `ClientEffect`
  - runtime work for adapters

This keeps crossterm and ratatui out of core.

## Proposed Extraction Order

## Phase 0: docs and naming

- Land this document.
- Freeze naming for the future core slices.
- Do not move code yet.

## Phase 1: introduce state slices inside the current crate

Create empty or lightly-populated modules:

- `src/client_core/mod.rs`
- `src/client_core/state/mod.rs`
- `src/client_core/state/conversation.rs`
- `src/client_core/state/composer.rs`
- `src/client_core/state/turn.rs`
- `src/client_core/state/stream.rs`
- `src/client_core/state/remote.rs`
- `src/client_core/state/workspace.rs`
- `src/client_core/state/surface.rs`
- `src/client_core/state/features.rs`
- `src/client_core/state/notices.rs`

Safe rule:

- move types first
- keep method bodies where they are until state compiles cleanly

## Phase 2: extract the easiest pure reducers

First extractions should be the least coupled files:

1. `state_ui_messages.rs`
2. `conversation_state.rs`
3. `state_ui_input_helpers.rs`
4. `remote/queue_recovery.rs`
5. `state_ui_maintenance.rs`

Why first:

- mostly state mutation
- low terminal/runtime coupling
- easy to cover with unit tests

## Phase 3: move workspace state into the app instance

This is the highest-leverage architectural fix.

Do this before large event-loop refactors:

1. replace `workspace_client.rs` global static state with `WorkspaceState` inside app/core
2. keep the same commands and behavior
3. adjust `remote/workspace.rs` to operate on instance-owned state

Why now:

- removes the clearest multi-surface blocker
- lowers future complexity for everything else

## Phase 4: extract remote event reduction

Split `src/tui/app/remote/server_events.rs` into:

- core reduction
  - state transitions
  - display-message mutations
  - token and tool-call accounting
  - status transitions
- runtime adapter
  - `RemoteEventState` parsing glue
  - redraw policy
  - transport-specific buffering

This is the single most important reducer extraction after workspace state.

## Phase 5: extract normalized terminal intents

Do not put raw `crossterm::Event` into core.

Instead:

1. keep key decoding in `local.rs`, `remote.rs`, and `input.rs`
2. introduce normalized intents such as:
   - `SubmitPrompt`
   - `MoveCursorLeft`
   - `ScrollChatUp`
   - `OpenSessionPicker`
   - `ToggleCopySelection`
   - `NavigateWorkspace(Direction)`
3. reduce those intents in core

## Phase 6: narrow the renderer boundary

Replace the current wide `TuiState` dependency with either:

- a much narrower trait, or
- a `PresentationSnapshot` built from core state

Recommended direction:

- build a `PresentationSnapshot` from core + presentation-owned caches

This keeps expensive derived computations out of ad hoc trait methods.

## Phase 7: move runtime adapters behind effects

Once reducers return `ClientEffect`, update:

- `src/tui/app/local.rs`
- `src/tui/app/remote.rs`
- `src/tui/app/remote/reconnect.rs`
- `src/tui/app/run_shell.rs`

to become thin shells that:

- collect external events
- reduce them
- run returned effects
- schedule redraws

## Phase 8: optional crate split

Only after ratatui/crossterm have been removed from core APIs:

- create `crates/jcode-client-core`
- move `src/client_core/*` into the crate
- keep presentation in the main crate or a future `jcode-tui-presentation` crate

Do not start with the crate split. Start with the boundary.

## Testing Strategy For The Split

Each extraction phase should preserve the existing user-visible behavior.

Recommended checks:

- existing TUI tests under `src/tui/ui_tests` and `src/tui/app/tests.rs`
- focused reducer tests for new `client_core` slices
- workspace state tests after de-globalizing `workspace_client.rs`
- remote `ServerEvent` reduction tests using captured event sequences

## Recommended First PR Sequence

If this work starts immediately, the first sequence should be:

1. docs only
   - this plan
2. type-only move
   - introduce `client_core::state::workspace::WorkspaceState`
   - no behavior change yet
3. safe behavioral move
   - make workspace state instance-owned
4. reducer move
   - extract `state_ui_messages.rs`
5. reducer move
   - extract `remote/server_events.rs`

That order minimizes risk while unlocking the most important future architecture work.

## Non-Goals For The First Split

Do not try to do these in the first wave:

- rewriting the renderer
- deleting the `TuiState` abstraction immediately
- moving mermaid/markdown rendering into core
- redesigning all overlays/widgets at once
- introducing a giant Redux-style universal action enum from day one
- making independent and workspace modes separate apps

## Bottom Line

The split should be:

- `client-core` = instance-owned client state + reducers + effects
- presentation = ratatui widgets, layout, drawing, render caches, visual debug

The safest first extraction is not a rendering change. It is making workspace state instance-owned and then extracting the existing pseudo-reducers, starting with display-message and remote-event reduction.
