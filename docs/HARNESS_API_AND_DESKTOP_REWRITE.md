# Harness as an API + Desktop Rewrite (Pure Rust)

Status: draft, approved direction (2026-07-24)

## Motivation

- The agent runtime ("harness") is already client/server: NDJSON over Unix
  socket (`~/.jcode/jcode.sock`), with `Request` / `ServerEvent` in
  `crates/jcode-protocol`. But it is an *internal* wire format: unversioned,
  ~147 variants, TUI-shaped, and coupled to client rendering assumptions.
- The current desktop app (`crates/jcode-desktop`, ~44k lines) hand-rolls
  rendering on wgpu 0.19 + winit 0.29 + glyphon, plus its own host/worker IPC.
  Text layout, rich text, scrolling, and markdown are all custom and are the
  main source of rendering quality problems. Rewrite from scratch.

## Part 1: Harness API

Goal: one stable, versioned boundary. Every UI (TUI, new desktop, future
web/mobile) is a client. No UI-specific logic in the runtime.

### Approach

Introduce `crates/jcode-harness-api`:

- **Versioned envelope.** Every frame carries `v` (protocol major) at the top
  level. Handshake: client sends `hello { min_version, max_version, client }`,
  server replies `hello_ok { version, server, capabilities }`. Unknown fields
  are ignored; unknown event types are skippable (tagged enums with a
  catch-all `Unknown` on the client side).
- **Curated surface, not a dump.** Start with a small stable core and grow:
  - Session lifecycle: create/attach/detach/list sessions, working dir.
  - Conversation: send message (text + images), cancel, soft interrupt,
    clear, rewind, history fetch.
  - Streaming events: text/reasoning deltas, tool start/input/exec/done,
    token usage, turn done, errors.
  - Permissions: permission request event + client response.
  - State: agent status snapshot, todos, plan/task-graph summaries.
  Everything else (swarm internals, selfdev, debug) stays on the internal
  protocol until promoted deliberately.
- **Transport.** NDJSON over Unix socket stays the primary transport.
  The API crate defines transport-agnostic types + a small client
  (`HarnessClient`) and server adapter, so a WebSocket/TCP transport can be
  added later without touching the schema.
- **Relationship to `jcode-protocol`.** Short term the server adapter maps
  API requests onto existing internal handling. Long term the internal
  protocol shrinks toward the API. Do not fork semantics: the API is a
  facade, the runtime remains the source of truth.

### Deliverables

1. `crates/jcode-harness-api`: types, version constants, handshake,
   `HarnessClient` (blocking + async-friendly framing).
2. Server: accept API handshake on the existing socket (sniff first line:
   `hello` = API client, else legacy).
3. Reference client example (`examples/harness_repl.rs`): connect, create
   session, send a message, print streamed events. This is the acceptance
   test for the API.
4. Schema snapshot test so accidental breaking changes fail CI.

## Part 2: Desktop rewrite (pure Rust)

Decision: **winit + wgpu + Vello + Parley** (Linebender stack), no UI
framework.

Why this middle ground:

- **Commodity rendering is library-backed.** Vello: GPU 2D vector renderer
  (paths, fills, glyph runs, clipping, layers). Parley: real text layout
  (shaping via swash, bidi, line breaking, rich text spans). These replace
  the two weakest hand-rolled parts of the old desktop.
- **Product-level composition stays fully custom.** We own the frame loop,
  scene graph, input routing, and animation. Niri-like control (tiling
  workspaces, gesture-driven spring transitions, per-surface transforms) is
  just "decide the transforms, emit the scene" each frame. No framework
  layout engine to fight.
- **Escape hatch to more custom.** Vello renders into a wgpu texture/surface
  we control. When we need effects Vello lacks (blur, shaders, custom
  compositing, embedding terminal grids), we add raw wgpu passes in the same
  frame. Start less custom, go more custom per-feature, never rewrite.

Explicitly rejected:
- egui: immediate mode fights smooth workspace animation and long rich
  transcripts.
- gpui: framework lock-in, Linux maturity, less compositor freedom.
- Keep current wgpu/glyphon code: the text/layout layer is the problem.

### Architecture sketch

```
jcode-desktop2 (new crate, old crate untouched until parity)
├── platform/     winit event loop, window, input, clipboard
├── gpu/          wgpu device/surface, Vello renderer, custom passes
├── scene/        retained scene: nodes with transform + content + z
├── text/         Parley layout cache, rich text (markdown -> spans)
├── anim/         springs/timelines driving node transforms (niri-style)
├── ui/           widgets built on scene+text (transcript, input, panels)
├── workspace/    tiling/workspace model, gestures, focus
└── harness/      HarnessClient wiring, session state, event -> ui model
```

Key invariants:
- The desktop talks to the runtime **only** through `jcode-harness-api`.
- Rendering is a pure function of (scene state, animation clock).
- Text layout is cached and invalidated by content/width changes only.

### Milestones

1. Harness API crate + handshake + reference client (validates Part 1).
2. `jcode-desktop2` skeleton: window, Vello "hello scene", Parley paragraph.
3. Transcript view: markdown -> rich spans -> Parley, smooth scrolling.
4. Input box + live streaming from harness API (first usable build).
5. Workspaces/tiling + spring animations (niri-like control).
6. Parity checklist vs old desktop, then delete `jcode-desktop`.
