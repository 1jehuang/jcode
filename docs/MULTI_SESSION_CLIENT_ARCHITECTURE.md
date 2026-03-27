# Multi-Session Client Architecture (Proposed)

Status: Proposed

This document describes a proposed evolution of jcode's UI architecture from the
current **single-session-per-client** model to a **multi-session-capable client**
model with built-in session workspace management.

The goal is to support a built-in spatial/multi-session UI for users on all
platforms, while preserving the current external-window workflow used with tools
like Niri.

See also:

- [`SERVER_ARCHITECTURE.md`](./SERVER_ARCHITECTURE.md)
- [`SWARM_ARCHITECTURE.md`](./SWARM_ARCHITECTURE.md)
- [`WINDOWS.md`](./WINDOWS.md)

## Summary

Today, jcode is effectively organized like this:

- **Server** owns many sessions.
- **Each client** usually attaches to one session.
- **Each terminal window/process** usually hosts one client.

That gives a practical mapping of:

- `session ≈ client ≈ process`

The proposed architecture changes the client model to:

- **Server** still owns many sessions.
- **Many clients** may still exist at once.
- **Each client may host one or many session surfaces**.

That changes the mapping to:

- `session = server-owned runtime`
- `surface = client-side attachment/view of a session`
- `client = container for one or many surfaces`

This preserves standalone windows while enabling a built-in multi-session
workspace.

## Goals

- Add a built-in multi-session workspace UI.
- Preserve the current standalone-client workflow.
- Preserve interoperability with external window managers like Niri.
- Make macOS and other platforms first-class for spatial multi-session use.
- Avoid duplicating the entire TUI stack into separate "standalone" and
  "workspace" apps.
- Keep the server as the source of truth for sessions.

## Non-Goals

- Replacing OS-level window managers.
- Building a general-purpose terminal multiplexer for arbitrary applications.
- Requiring all users to adopt workspace mode.
- Supporting fully concurrent editing from multiple interactive attachments to the
  same session in the first version.

## Current Architecture

Current high-level model:

```text
Server
  ├── Session A
  ├── Session B
  └── Session C

Client 1 -> Session A
Client 2 -> Session B
Client 3 -> Session C
```

In practice, each client is typically its own terminal window/process, so users
who want a spatial layout today rely on an external window manager.

This works well on Linux with tools like Niri, but is not portable enough for a
cross-platform built-in workspace experience.

## Proposed Architecture

### Core idea

The server continues to own sessions, but the client evolves from a
single-session UI into a **multi-session shell**.

```text
Server
  ├── Session A
  ├── Session B
  ├── Session C
  └── Session D

Client 1 (workspace)
  ├── Surface A -> Session A
  ├── Surface B -> Session B
  └── Surface C -> Session C

Client 2 (standalone)
  └── Surface D -> Session D
```

A standalone window becomes just a client hosting one surface. A workspace
becomes a client hosting many surfaces.

## Terminology

### Session

A server-owned runtime containing:

- conversation history
- provider/model state
- tool execution state
- session persistence
- background task state
- memory extraction state

A session is **not** fundamentally a window or process.

### Surface (or Attachment)

A client-side interactive or passive view of a session.

Examples:

- a session shown inside the built-in workspace
- a standalone jcode window attached to one session

A surface is the UI representation of a session in a specific client.

### Client

A TUI process that hosts one or many surfaces.

Examples:

- current standalone jcode window
- future multi-session workspace client

## Key Design Rule

The architecture must separate:

### Shared session state

Owned by the server:

- messages
- streaming/tool events
- model/provider selection
- persisted metadata
- background execution state
- server-side session lifecycle

### Surface-local UI state

Owned by a specific client surface:

- input draft
- cursor position
- scroll position
- selection/copy state
- local pane focus
- pane zoom/fullscreen state
- local viewport and layout placement

This separation is required to support:

- one session shown in different places over time
- popping a session out into a standalone window
- docking a standalone session back into a workspace
- different local view state for the same underlying session

## Client Modes

The same client binary should support two primary modes.

### Single-surface mode

Equivalent to today's standalone client:

- one client
- one surface
- one session attached

This should remain the default/simple mental model for many users.

### Multi-surface mode

Workspace mode:

- one client
- many surfaces
- spatial navigation and session management built in

This mode provides the in-app session manager and workspace UI.

## Interoperability with External Window Managers

Preserving interop with Niri and similar tools is a core requirement.

The built-in workspace must not replace standalone clients. Instead, both should
remain first-class.

### Required workflow support

- attach a session inside the in-app workspace
- pop a session out into its own standalone client/window
- optionally dock a standalone session back into a workspace
- allow multiple standalone clients to coexist with a workspace client

### Resulting model

- many clients may exist at once
- each client may host one or many session surfaces
- the server still owns the underlying sessions

## Interaction Ownership

For an initial implementation, a session should have **one active interactive
surface** at a time.

That means:

- if a workspace surface is popped out into a standalone window, the standalone
  surface becomes the active interactive owner
- the workspace surface should either disappear or become passive
- docking reverses that ownership

This avoids synchronization problems with:

- multiple input drafts
- racing submissions
- cursor/focus conflicts
- duplicate interactive ownership of the same session

A future design may allow richer mirroring or passive previews, but v1 should
prefer a single active controller per session.

## Client-Side Architecture

The current single `App` object is too monolithic to scale cleanly to many
sessions. The client should be split into layers.

### `ClientShell`

Global process/UI state:

- terminal event loop
- workspace layout
- focus management
- keyboard mode (normal/insert/command)
- surface management
- pop-out / dock orchestration
- global commands and notifications

### `SessionController`

Per-session live controller:

- subscribe/resume session
- submit message
- cancel current turn
- apply model/session commands
- receive and apply server events
- reconnect logic

### `SessionSurfaceState`

Per-surface local UI state:

- input buffer
- cursor position
- scroll state
- selection/copy state
- side pane local viewport
- local focus and zoom state

### Shared session renderer

A reusable rendering layer that can render a session surface into an arbitrary
rect. This is the key step for making both standalone and workspace modes reuse
one UI stack.

## Suggested Internal Model

```rust
struct ClientShell {
    surfaces: Vec<SessionSurface>,
    focused_surface: Option<SurfaceId>,
    mode: ClientMode,
    layout: LayoutState,
}

struct SessionSurface {
    surface_id: SurfaceId,
    session_id: SessionId,
    controller: SessionController,
    ui: SessionSurfaceState,
}

struct SessionController {
    // v1: dedicated remote connection per surface
    // v2: multiplexed session handle
}

struct SessionSurfaceState {
    input: String,
    cursor_pos: usize,
    scroll_offset: usize,
    side_pane_focus: bool,
    zoomed: bool,
}
```

This enables:

- standalone mode = one-surface client
- workspace mode = many-surface client

## Transport / Protocol Strategy

### Phase 1: dedicated connection per active surface

Fastest practical path:

- one client process
- one remote connection per live session surface

Pros:

- minimal protocol changes
- reuses the current session-oriented client behavior
- easiest way to prove out workspace UX

Cons:

- more overhead per hosted surface
- duplicate connection/reconnect machinery inside one process
- not the cleanest long-term abstraction

### Phase 2: multiplexed client protocol

Longer-term architecture:

- one client connection can subscribe to many sessions
- requests and events are explicitly tagged by `session_id`

Examples:

```rust
Request::SendMessage { session_id, ... }
Request::Cancel { session_id, ... }
ServerEvent::TextDelta { session_id, text }
ServerEvent::Done { session_id, ... }
```

Pros:

- cleaner workspace-native design
- lower connection overhead
- clearer event routing for multi-session clients

Cons:

- larger protocol and server refactor

Recommendation: do not block v1 on protocol multiplexing.

## Pop-Out / Dock Workflows

### Pop out to standalone window

1. User selects a workspace surface.
2. Client spawns a standalone jcode client attached to the same session.
3. Standalone surface becomes the active interactive owner.
4. Workspace surface is removed or downgraded to passive.

### Dock into workspace

1. User requests dock for a standalone session.
2. Workspace client creates a surface for that session.
3. Workspace surface becomes active interactive owner.
4. Standalone client exits or detaches.

## Interop API Surface

The architecture should expose a small control surface for external and internal
interop.

Potential operations:

- `list_sessions`
- `list_surfaces`
- `workspace_state`
- `focus_session(session_id)`
- `open_session_in_window(session_id)`
- `dock_session(session_id)`
- `undock_session(session_id)`
- `move_session_to_workspace(session_id, position)`

This can initially be provided through existing jcode control channels such as:

- CLI commands
- the main server protocol
- debug/control socket

The exact public API shape is less important than preserving a clean internal
model for these operations.

## Recommended UI Direction

For a first version, prefer a **columnar or tiled workspace** over a fully
freeform floating system.

Reasons:

- closer to the Niri mental model
- easier keyboard navigation
- simpler implementation
- easier to render in a terminal UI
- better first step toward a scrollable spatial workspace

A more freeform or richer 2D canvas can be layered on later if needed.

## Migration Plan

### Phase 0: renderer extraction

- Extract a reusable session rendering layer from the current TUI.
- Stop assuming one `App` owns the entire terminal surface.

### Phase 1: surface/controller split

- Split current monolithic client state into shell/controller/surface layers.
- Keep single-surface behavior unchanged.

### Phase 2: multi-surface client

- Allow one client process to host multiple session surfaces.
- Add basic tiled/columnar workspace UI.
- Add focus movement and session add/remove operations.

### Phase 3: pop-out support

- Add commands to open a hosted session in a standalone client.
- Preserve current `jcode --resume <session>` workflow.

### Phase 4: dock support

- Allow a standalone session to be reattached into a workspace client.
- Keep one interactive owner per session.

### Phase 5: protocol cleanup

- Evaluate session-multiplexed protocol support.
- Replace dedicated per-surface connections if and when it is clearly beneficial.

## Open Questions

- Should passive mirrored surfaces exist in v1, or should a session exist in only
  one visible place at a time?
- Which pieces of side-panel state are session-scoped vs surface-scoped?
- Should workspace mode be a new command (`jcode workspace`) or a runtime mode of
  the normal client?
- How should dock/undock be exposed: command palette, slash commands, CLI, debug
  socket, or all of the above?
- How much workspace layout state should be persisted across launches?

## Recommendation

Adopt the following design direction:

1. **Expand the client to support multiple session surfaces.**
2. **Keep the server as the owner of sessions.**
3. **Preserve standalone clients as first-class.**
4. **Treat workspace panes and standalone windows as different surfaces for the
   same session model.**
5. **Start with one active interactive surface per session.**
6. **Prototype with one connection per active surface before attempting protocol
   multiplexing.**

This gives jcode a portable built-in multi-session workspace without sacrificing
existing workflows or external window-manager interop.
