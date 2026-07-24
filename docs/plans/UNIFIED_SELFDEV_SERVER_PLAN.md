# Unified Self-Dev / Normal Server Plan

> Status: **Implemented.**
>
> This document is preserved as a historical design/rollout plan. The current
> architecture uses a single shared server, with self-dev handled as a
> session-local canary capability rather than a separate dedicated daemon/socket.
> Any references below to `/tmp/jcode-selfdev.sock`, `canary-wrapper`, or
> `JCODE_SELFDEV_MODE` describe the pre-merge architecture or transition steps,
> not the current runtime design.

## Goal

Reduce RAM usage by removing the dedicated self-dev daemon/socket pair and treating self-dev as a **session capability** on the normal shared server.

Today, normal sessions and self-dev sessions can end up with separate long-lived server processes, which duplicates:

- Tokio runtime overhead
- allocator heap / fragmentation footprint
- MCP pool state
- embedding/model lifecycle machinery
- event buffers, registries, session maps, swarm maps
- general server baseline RSS

## Current Architecture

### Normal mode
- Main socket: runtime `jcode.sock`
- Debug socket: runtime `jcode-debug.sock`
- Startup path: `jcode` -> default client flow -> spawn `jcode serve` if needed

### Self-dev mode
- Main socket: `/tmp/jcode-selfdev.sock`
- Debug socket: `/tmp/jcode-selfdev-debug.sock`
- Startup path:
  - repo auto-detection or `jcode self-dev`
  - `cli/selfdev.rs::run_self_dev()`
  - exec into `canary-wrapper`
  - wrapper ensures self-dev server exists on dedicated socket
  - wrapper launches TUI client against that socket

## Key Finding From Code Inspection

The runtime already supports **per-session self-dev state**:

- protocol `Subscribe { working_dir, selfdev }`
- server subscribe handling can mark only that session as canary/self-dev
- `selfdev` tool availability is already gated on `session.is_canary`
- prompt additions are already gated on `session.is_canary`
- clear/resume/headless flows already preserve or infer canary state per session

This means the main remaining split is not the session model, but the **startup / reload / wrapper plumbing**.

## Target Architecture

### One shared server
- Main socket: runtime `jcode.sock`
- Debug socket: runtime `jcode-debug.sock`
- Self-dev sessions connect to the same server as normal sessions

### Self-dev becomes session-local
A client is self-dev if any of the following are true:
- explicit `jcode self-dev`
- current working directory is the jcode repo (auto-detected)
- resumed session is already canary

That client connects to the shared server and sends:
- `working_dir`
- `selfdev: true`

The server then:
- marks the session canary
- registers selfdev tools for that session
- includes selfdev prompt additions for that session only

### Debug socket
With one shared server, there is one shared debug socket.

Consequences:
- no dedicated self-dev debug socket
- debug tooling sees both normal and self-dev sessions from the same server
- selfdev-sensitive actions remain gated by target session canary state

## Important Policy Decision

If a self-dev session triggers a reload, it reloads the **shared server**.
That means all clients reconnect.

This is the cleanest design for RAM savings.

The binary chosen for reload should depend on the **triggering session**, not a server-global self-dev mode flag:

- normal session reload -> stable / launcher candidate
- canary session reload -> repo / canary candidate

## Implementation Phases

### Phase 1 - Client-side self-dev on shared server path
**Goal:** stop repo auto-detection from forcing a separate self-dev daemon.

Changes:
- do not auto-divert repo startup into `canary-wrapper`
- introduce a client-only self-dev signal (separate from server self-dev env)
- keep using normal server spawn/connect path
- continue sending `Subscribe { selfdev: true }`
- prevent the shared server child process from inheriting the client-only self-dev env
- stop server self-dev detection from inferring self-dev based on current working directory

Expected result:
- opening jcode inside the repo uses the shared server path by default
- session still becomes canary/self-dev
- explicit `jcode self-dev` command may still use legacy wrapper temporarily

### Phase 2 - Move explicit `jcode self-dev` onto shared server path
**Goal:** make explicit self-dev command use the same shared-server flow.

Changes:
- simplify `cli/selfdev.rs::run_self_dev()`
- keep optional `cargo build --release`
- set client-only self-dev mode
- connect through normal client/server startup path
- remove need for `canary-wrapper` in standard usage

Expected result:
- both auto-detected self-dev and explicit `jcode self-dev` share one server

### Phase 3 - Session-targeted reload selection
**Goal:** remove server-global self-dev assumptions from reload/update behavior.

Changes:
- include triggering session context in reload handling
- choose server exec target based on triggering session canary state
- always run reload monitor on the shared server, but authorize via session state / request policy

Expected result:
- one shared server can still reload into the right binary

### Phase 4 - Remove dedicated self-dev socket assumptions
**Goal:** fully retire the separate socket model.

Changes:
- deprecate `/tmp/jcode-selfdev.sock` and `/tmp/jcode-selfdev-debug.sock`
- update docs, tests, and scripts that probe self-dev via separate sockets
- simplify debug/test tooling to use the shared debug socket

## Risks / Tradeoffs

### Shared reload impact
A self-dev-triggered reload affects all clients on the shared server.
This is the main behavior change and the key tradeoff for RAM savings.

### Legacy tooling assumptions
Some scripts and tests currently prefer the self-dev debug socket path and will need updating.

### Scattered env-based logic
There are multiple `JCODE_SELFDEV_MODE` checks across startup, hot reload, and server behavior; these need to be separated into:
- client self-dev request
- server self-dev mode (legacy / compatibility)
- session canary capability

## Files Likely To Change

- `src/cli/dispatch.rs`
- `src/cli/selfdev.rs`
- `src/cli/hot_exec.rs`
- `src/server.rs`
- `src/server/reload.rs`
- `src/server/client_session.rs`
- `src/tui/mod.rs`
- `src/tui/backend.rs`
- `docs/SERVER_ARCHITECTURE.md`
- debug/test scripts that assume separate self-dev sockets

## Recommended Order

1. Land Phase 1 foundations and shared-path client self-dev
2. Land explicit `jcode self-dev` shared-path behavior
3. Refactor reload/update selection to be session-targeted
4. Remove legacy wrapper/socket assumptions and update tests/docs
