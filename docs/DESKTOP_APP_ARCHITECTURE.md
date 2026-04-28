# Jcode Desktop Architecture Direction

Status: Design direction, active prototype in progress
Updated: 2026-04-28

This document captures the initial direction for a desktop application for Jcode under these constraints:

- no Electron/Tauri/web-app shell
- no general UI framework
- very high performance
- low idle resource use
- very custom product UI
- primary developer machine may be Linux
- most early users are expected to be on macOS

The goal is to make the desktop client a first-class Jcode surface without forking the Jcode runtime or turning the app into a heavyweight IDE clone.

See also:

- [`DESKTOP_SUPERAPP_WORKSPACE.md`](./DESKTOP_SUPERAPP_WORKSPACE.md)
- [`DESKTOP_CODEBASE_ARCHITECTURE.md`](./DESKTOP_CODEBASE_ARCHITECTURE.md)
- [`CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md`](./CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md)
- [`MULTI_SESSION_CLIENT_ARCHITECTURE.md`](./MULTI_SESSION_CLIENT_ARCHITECTURE.md)
- [`SERVER_ARCHITECTURE.md`](./SERVER_ARCHITECTURE.md)
- [`DESKTOP_SINGLE_SESSION_DESIGN.md`](./DESKTOP_SINGLE_SESSION_DESIGN.md)
- [`OPENAI_APPS_MCP_ASSISTANT_REVIEW.md`](./OPENAI_APPS_MCP_ASSISTANT_REVIEW.md)
- [`MEMORY_BUDGET.md`](./MEMORY_BUDGET.md)

## Executive summary

Build Jcode Desktop as a small Rust desktop client with a custom GPU-rendered UI. The app connects to a local Jcode server/daemon that owns sessions, tools, agent execution, persistence, and permissions. As of 2026-04-28, the single-session desktop path is an active prototype with server-backed chat, assistant markdown rendering, model/session overlays, image attachments, tool cards, selection/copy, and stdin-response handling.

The frontend should be optimized as a render/input surface:

- Linux should be a first-class development platform.
- macOS should be the first-class product/distribution platform.
- The UI should not depend on Linux-only desktop concepts.
- The UI should not be a web view.
- The UI should not embed the agent runtime directly.
- Rendering should be on-demand, virtualized, and measurable from day one.

Recommended initial stack:

| Area | Decision |
|---|---|
| Frontend language | Rust |
| Backend/runtime | Existing Rust Jcode server/session runtime |
| Process model | Desktop frontend + local Jcode daemon/server |
| Window/input layer | Thin platform layer, likely `winit` initially |
| Rendering | `wgpu` with a custom 2D renderer |
| UI architecture | Retained UI tree with dirty tracking |
| Layout | Small custom layout system, not CSS/DOM |
| Text | Dedicated text layout/raster cache, likely `cosmic-text`/`swash` or platform-backed text later |
| Protocol | Versioned typed local event protocol |
| Persistence | Server-owned session/event persistence |
| Product identity | Agent operating console / mission control |

## Product stance

Jcode Desktop should not start as a full IDE and should not look like a conventional chatbot.

The differentiated product is a **keyboard-driven, Niri-like agent workspace superapp** for local development. The first-class object is not a chat window, but a workspace containing many navigable surfaces:

- agent sessions
- activity/task views
- diffs and changed files
- file/diff/tool surfaces
- optional future surfaces
- settings/debug/tool surfaces

The app should help users:

- supervise autonomous coding work
- inspect tool activity
- manage background tasks
- review changed files
- respond to permission prompts
- resume and coordinate sessions
- navigate many related surfaces spatially

The desktop client should complement the TUI/CLI, not replace it.

## Platform strategy

### Development host: Linux

Linux should support the fastest inner loop:

- launch the desktop client locally
- run renderer stress tests
- run protocol integration tests
- benchmark memory/frame/layout/text performance
- debug the UI engine without a Mac in the loop

The Linux build should be real, not a fake simulator. It should render through the same UI engine and exercise the same protocol/view-model paths as macOS.

### Product target: macOS first

Most early users are expected to be on macOS, so macOS polish should be a product requirement even if day-to-day development happens on Linux.

Mac-specific work that should not be postponed too long:

- native `.app` bundle
- app icon and menu bar integration
- command-key shortcuts
- system light/dark appearance
- Retina rendering correctness
- trackpad scrolling quality
- native clipboard behavior
- file/open-with integration
- code signing and notarization path
- good behavior under Mission Control, Spaces, and full-screen windows

### Avoid Linux-shaped product assumptions

Because the developer may use Linux, the architecture should explicitly avoid baking in assumptions that work well only with a Linux window manager.

Do not make these hard dependencies:

- Niri-style external spatial window management
- X11-specific APIs
- Wayland-only behavior
- terminal-first session workflows
- Linux notification semantics
- global shortcuts that are unavailable or hostile on macOS

The existing Linux/Niri workflow should remain excellent, but desktop product quality should be judged primarily against macOS expectations.

## Process architecture

Use a split process architecture:

```text
Jcode Desktop Frontend
  - window/input
  - custom rendering
  - local view model
  - transient UI state
  - surface-local state
  - protocol client

Jcode Server/Daemon
  - sessions
  - agent runtime
  - tool runtime
  - background tasks
  - persistence
  - permissions
  - model/provider configuration
```

The server remains the source of truth for:

- canonical session history
- streaming events
- tool execution
- file edits
- background tasks
- permission state
- persisted configuration

The desktop frontend owns only surface-local state:

- selected session/surface
- draft input
- cursor and text selection
- scroll offsets
- pane sizes
- focused panel
- local command palette state
- render caches

This aligns with the multi-session model where a server-owned session can be shown by different clients or surfaces over time.

## Local protocol direction

The desktop app should consume a versioned, typed event stream rather than periodically fetching complete session snapshots.

Early protocol properties:

- local-first transport
- explicit protocol version
- capability negotiation
- append-only session events
- streaming deltas for assistant/tool output
- resumable subscriptions by event cursor
- compact events for high-volume tool output
- server-owned permission requests

Possible transports:

1. Existing Jcode server channel, if compatible with desktop needs.
2. Unix domain socket on Linux/macOS and named pipe on Windows.
3. Stdio JSON protocol for early prototypes and test harnesses.

Avoid localhost HTTP as the default unless there is a strong reason. It creates a larger local security surface than a user-owned socket/pipe.

Example event families:

```text
session.created
session.updated
surface.attached
message.created
message.delta
message.completed
tool.started
tool.output.delta
tool.completed
task.started
task.progress
task.completed
workspace.changed
git.changed
permission.requested
permission.resolved
error
```

## Rendering architecture

Use a custom renderer rather than a native widget hierarchy or web view.

Recommended layers:

```text
Platform window/input
  -> input normalizer
  -> app state/view model
  -> retained UI tree
  -> layout
  -> text layout/cache
  -> display list
  -> GPU renderer
```

Core rules:

- no continuous render loop when idle
- render only on input, data events, animations, or explicit invalidation
- virtualize every unbounded list
- separate layout cost from paint cost
- cache shaped text by content/font/width
- use stable IDs for dirty tracking
- make debug/performance counters visible in-app

The renderer should initially support:

- rectangles
- rounded rectangles
- borders
- solid fills
- clipping
- scroll containers
- text runs
- monospaced blocks
- simple icons or vector-like primitives
- image support later

Defer:

- blur effects
- complex shadows
- animation framework
- SVG-heavy rendering
- full markdown renderer
- full terminal emulator
- embedded code editor

## UI architecture

Use a retained UI tree with immediate-style builder ergonomics.

Rationale:

- transcripts are long-lived and streamed incrementally
- tool outputs can be large
- panes need stable focus/selection state
- dirty tracking matters for resource use
- accessibility will eventually need stable semantic nodes
- multi-session surfaces need stable identity

The model should not imitate the DOM/CSS stack. A small product-specific layout system is enough:

- row
- column
- stack
- split pane
- fixed size
- flex fill
- scroll container
- virtual list
- overlay/modal
- intrinsic text measurement

## Text strategy

Text is one of the hardest parts of this project and should be treated as a core system, not a detail.

The desktop client needs:

- Unicode shaping
- font fallback
- monospace code/tool output
- wrapping
- incremental append layout
- selection/copy
- input cursor behavior
- command palette text input
- markdown-ish transcript styling
- ANSI-like tool output styling eventually

Initial recommendation:

- use a Rust text stack such as `cosmic-text`/`swash` if dependency review is acceptable
- maintain a GPU glyph atlas
- cache shaped lines/runs by stable block ID and available width
- specialize streamed append paths so new output does not re-layout the whole transcript

Mac-specific text quality should be evaluated early. If Rust text rendering is not good enough on macOS, consider platform-backed text for macOS while preserving the same higher-level text layout API.

## Performance and resource budgets

Initial budgets should be measured on both Linux development machines and representative macOS hardware.

| Metric | MVP target | Long-term target |
|---|---:|---:|
| Cold launch to visible window | < 500 ms | < 150 ms |
| Frontend idle CPU | ~0% | ~0% |
| Frontend idle RSS | < 100 MiB | < 50 MiB |
| Input-to-paint latency | < 32 ms | < 16 ms |
| Scrolling | 60 fps | 120 fps-capable |
| Fake transcript stress case | 100k blocks usable | 100k blocks smooth |
| Full transcript re-layout on append | forbidden | forbidden |
| Unbounded retained visible nodes | forbidden | forbidden |
| Renderer frame when idle | forbidden | forbidden |

Required early instrumentation:

- frame time
- layout time
- text shaping time
- display-list build time
- GPU submit time
- visible node count
- total retained node count
- glyph atlas size
- text cache size
- protocol event backlog
- daemon round-trip latency
- frontend RSS if available

A debug HUD should exist in the prototype before real Jcode integration is considered complete.

Example HUD:

```text
frame 1.8ms | layout 0.3ms | text 0.6ms | gpu 0.4ms
nodes 812 | visible 47 | glyph atlas 12.4 MiB | events 0 | daemon 2ms
```

## MVP scope

The first UI milestone should prove the engine before proving every product workflow.

### Milestone 1: custom shell with fake data

Success criteria:

- launches a native desktop window from Linux
- renders through the custom GPU pipeline
- shows session sidebar, transcript, composer, and activity panel
- handles mouse, keyboard, focus, and scrolling
- renders fake streamed transcript data
- virtualizes a 100k-block transcript
- idles at near-zero CPU
- exposes performance/debug HUD
- has screenshot or golden-state tests where practical

### Milestone 2: protocol connection

Success criteria:

- connects to local Jcode server/daemon
- lists sessions
- attaches to a session/surface
- subscribes to event stream
- sends a user prompt
- streams assistant/tool events into the transcript
- can stop/cancel an active run
- recovers from daemon restart or disconnect gracefully enough for development use

### Milestone 3: useful agent console

Success criteria:

- activity center for background tasks/tool calls
- permission request overlay
- workspace/git status panel
- changed-file list
- open external editor/diff action
- session search/filter
- macOS app bundle prototype

## Crate layout proposal

Do not put the whole desktop app in the root crate.

Suggested structure:

```text
crates/
  jcode-desktop-protocol/   # shared protocol/event types if not already covered by server types
  jcode-desktop-ui/         # UI tree, layout, text/cache abstractions, renderer-agnostic pieces
  jcode-desktop-renderer/   # wgpu renderer and GPU resources
  jcode-desktop/            # app shell, platform window, protocol client, product UI
```

If compile time becomes a problem, keep protocol/UI crates lightweight and gate GPU/window dependencies behind the final app crate.

## Dependency policy

“No frameworks” does not have to mean “no libraries.” It should mean no heavyweight app framework and no web-shell product architecture.

Likely acceptable dependencies:

- `wgpu` for rendering abstraction
- a very thin window/input layer such as `winit` for bootstrapping
- `cosmic-text`/`swash` or equivalent for text shaping/rasterization
- small serialization/protocol crates already consistent with Jcode

Avoid:

- Electron
- Tauri
- Qt
- Flutter
- GTK as the app framework
- WebView UI shell
- React/Vue/Svelte-style UI stack
- CSS/DOM-based architecture

If `winit` becomes limiting for macOS polish, the platform layer can grow direct AppKit support while preserving the renderer and UI model.

## macOS validation checklist

Because macOS is the primary user target, validate these early even if development happens on Linux:

- Retina scale factor correctness
- trackpad inertial scrolling
- text clarity compared with native apps
- keyboard shortcuts use Command rather than Control where appropriate
- system dark/light mode follows user preference
- window resizing and full-screen behavior feels native
- app menu and close/minimize/quit semantics are correct
- clipboard round-trips rich enough for code and transcripts
- local socket permissions are safe
- app bundle can launch/find the daemon reliably

## Open decisions

These should be resolved before implementation moves past the fake-data prototype:

1. Use `winit` initially or write direct platform shells from the start?
2. Use `wgpu` or direct Metal-first rendering?
3. Use `cosmic-text`/`swash` or platform text APIs?
4. Reuse the existing Jcode server protocol or introduce a desktop-specific event protocol crate?
5. Should the first desktop binary support multi-surface mode or only one active surface?
6. What is the minimum macOS version to support?
7. What is the first distribution path: local `.app`, Homebrew cask, or signed/notarized DMG?

## Recommended immediate next step

Create a fake-data desktop prototype that runs on Linux but measures the exact performance characteristics required by the eventual macOS product.

The prototype should not wait for a perfect daemon API. It should validate the expensive UI systems first:

- window creation
- renderer startup
- retained tree
- layout
- text cache
- virtualized transcript
- on-demand repaint
- debug HUD

Only after that should the real Jcode event stream be connected.
