# Desktop Codebase Architecture from the Existing TUI

> Historical: this document describes the removed legacy `crates/jcode-desktop` app. The current desktop app is `crates/jcode-desktop2`.

Status: Proposed
Updated: 2026-04-25

This document translates the current Jcode TUI architecture into a concrete codebase plan for a future custom desktop app.

The desktop app is expected to have roughly the same product capabilities as the TUI, but it should not be a direct port of the TUI implementation. The TUI is terminal/cell-oriented and has accumulated a large amount of terminal-specific rendering, input, layout, scrolling, and cache logic. The desktop app should reuse the runtime/protocol/session concepts and some presentation models, but it should have a separate custom UI and rendering architecture.

See also:

- [`DESKTOP_APP_ARCHITECTURE.md`](./DESKTOP_APP_ARCHITECTURE.md)
- [`DESKTOP_SUPERAPP_WORKSPACE.md`](./DESKTOP_SUPERAPP_WORKSPACE.md)
- [`CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md`](./CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md)
- [`MULTI_SESSION_CLIENT_ARCHITECTURE.md`](./MULTI_SESSION_CLIENT_ARCHITECTURE.md)
- [`SERVER_ARCHITECTURE.md`](./SERVER_ARCHITECTURE.md)

## Current TUI observations

The current TUI is feature-rich and should be treated as the product reference implementation.

Approximate current size:

```text
src/tui/*.rs and submodules: 144 Rust files, ~115k lines
src/tui/app.rs:             ~800 lines
src/tui/ui.rs:              ~3.8k lines
src/tui/ui_prepare.rs:      ~1.6k lines
src/tui/ui_viewport.rs:     ~750 lines
src/tui/ui_messages.rs:     ~2.4k lines
src/tui/markdown.rs:        ~1.4k lines
src/protocol.rs:            ~1.4k lines
```

Important existing pieces:

- `src/protocol.rs`
  - newline-delimited JSON over Unix socket
  - `Request`
  - `ServerEvent`
  - session subscribe/resume/history/message/cancel/tool/status events
- `src/server/`
  - multi-client server/session runtime
  - reconnect support
  - session lifecycle
  - client events
  - background tasks/swarm/comm state
- `src/tui/app.rs` and `src/tui/app/*`
  - TUI app state root
  - input handling
  - remote connection handling
  - command parsing
  - server event reducer
  - local mode support
  - copy/selection/navigation/session picker/debug overlays
- `src/tui/ui.rs` and `src/tui/ui_*`
  - ratatui renderer
  - terminal/cell layout
  - viewport and scroll behavior
  - side panes
  - overlays
  - visual debug capture
- `src/tui/ui_prepare.rs`
  - frame preparation
  - wrapped line maps
  - full prep cache
  - body/streaming/batch preparation
- `src/tui/ui_messages.rs`
  - message-to-terminal-line rendering
  - tool/system/background/swarm usage rendering
  - line caches
- `src/tui/markdown.rs`
  - terminal markdown rendering
  - syntax highlighting cache
  - mermaid integration hooks

## Key lesson

The TUI already has the right **feature set** and many correct **domain concepts**, but it does not yet have the right boundaries for a custom desktop UI.

The desktop should not import `ratatui::Line`, terminal-width wrapping, global renderer caches, or terminal input assumptions into core app state.

The desktop should instead use this split:

```text
Jcode server/runtime/protocol
  -> client-core reducer and view model
    -> desktop product views
      -> custom UI tree/layout
        -> display list
          -> wgpu renderer
```

## What to reuse versus not reuse

### Reuse directly or almost directly

- server process architecture
- session ownership model
- reconnect semantics
- request/event protocol as the starting point
- server-side session history and tool execution
- model/provider/session metadata
- command concepts
- background task concepts
- swarm/goal/activity concepts
- permission concepts
- debug/diagnostic philosophy

### Reuse after extracting away terminal types

- server event reduction logic from `src/tui/app/remote/server_events.rs`
- message display block construction
- tool call summaries
- activity/status models
- markdown block parsing decisions
- copy target semantics
- session picker data model
- login/account picker data model
- command registry and command metadata
- info widget data models
- memory/debug summary models

### Do not reuse directly

- `ratatui::Line` as a cross-surface representation
- terminal cell layout
- terminal-specific scroll offsets as the primary desktop model
- global renderer state such as `LAST_MAX_SCROLL`-style globals
- terminal key protocol code
- terminal-specific markdown wrapping
- terminal-specific image/mermaid display code
- the giant `TuiState` trait as the desktop boundary
- monolithic `App` state with runtime, transport, UI, and render concerns mixed together

## The main architectural risk

If desktop development copies the TUI structure directly, the result will likely be:

- another very large `DesktopApp` state object
- rendering caches mixed with domain state
- platform input handling mixed with session reducers
- duplicated command logic
- duplicated server event handling
- hard-to-test UI behavior
- difficulty sharing behavior between TUI and desktop

The desktop app should avoid repeating this by creating a real client-core boundary before implementing too many features.

## Proposed crate/module architecture

The exact crate names can change, but the dependency direction should not.

```text
crates/
  jcode-protocol/             # eventually extracted from src/protocol.rs
  jcode-client-core/          # surface-independent client state/reducers/view models
  jcode-desktop-ui/           # custom UI tree, layout, input routing, style tokens
  jcode-desktop-renderer/     # wgpu renderer, display list, glyph/image atlases
  jcode-desktop-platform/     # winit/AppKit/Linux shell abstraction, menus, clipboard
  jcode-desktop/              # product app: windows, panels, protocol client, composition
```

Initial implementation may keep some of these as modules inside `crates/jcode-desktop` to reduce early friction, but the boundaries should be designed as if they were separate crates.

## Dependency direction

Allowed dependency flow:

```text
jcode-desktop
  -> jcode-desktop-platform
  -> jcode-desktop-renderer
  -> jcode-desktop-ui
  -> jcode-client-core
  -> jcode-protocol
```

`jcode-client-core` must not depend on:

- `wgpu`
- `winit`
- AppKit
- Wayland/X11
- `ratatui`
- `crossterm`
- terminal markdown rendering

`jcode-desktop-ui` should not depend on the Jcode server runtime. It can depend on client-core view models and generic UI types.

`jcode-desktop-renderer` should not know what a Jcode session is. It renders display lists, text runs, images, clips, and primitives.

## `jcode-protocol`

The existing `src/protocol.rs` is already the natural starting point.

Long-term, extract it into a crate so both TUI and desktop consume the same wire types:

```text
crates/jcode-protocol/src/lib.rs
  Request
  ServerEvent
  HistoryMessage
  SessionActivitySnapshot
  FeatureToggle
  SwarmMemberStatus
  protocol version/capability handshake types
```

Short-term, desktop can import the root crate types, but the protocol should be treated as shared API.

Desktop-specific protocol needs may include:

- session list with metadata optimized for a sidebar
- event cursors for resumable subscriptions
- richer activity snapshots
- workspace/git summary snapshots
- permission request snapshots
- changed-file summaries
- surface/window attachment metadata

Avoid making a second unrelated desktop protocol unless the existing protocol becomes a blocker.

## `jcode-client-core`

This is the most important shared layer.

It should own behavior and state that are independent of the terminal and independent of desktop rendering.

Suggested modules:

```text
jcode-client-core/
  src/
    lib.rs
    app_model.rs
    actions.rs
    reducer.rs
    protocol_reducer.rs
    command_registry.rs
    session_list.rs
    transcript.rs
    transcript_blocks.rs
    composer.rs
    activity.rs
    permissions.rs
    workspace.rs
    git.rs
    settings.rs
    overlays.rs
    selection.rs
    status.rs
    diagnostics.rs
    view_model.rs
```

### Core state slices

```rust
struct ClientCore {
    sessions: SessionListState,
    active_surface: Option<SurfaceId>,
    surfaces: SurfaceMap,
    connection: ConnectionState,
    commands: CommandRegistry,
    activity: ActivityState,
    permissions: PermissionState,
    workspace: WorkspaceState,
    diagnostics: DiagnosticsState,
}
```

Each surface owns local UI state:

```rust
struct SurfaceState {
    session_id: SessionId,
    transcript: TranscriptState,
    composer: ComposerState,
    selection: SelectionState,
    scroll: ScrollState,
    focused_region: FocusRegion,
    overlays: OverlayStack,
    pane_layout: PaneLayoutState,
}
```

The important rule:

> Server-owned session state and surface-local UI state must remain separate.

This matches the existing multi-session architecture docs and makes desktop multi-window/multi-surface possible later.

### Actions and reducers

Desktop and TUI should eventually share reducer logic through typed actions:

```rust
pub enum ClientAction {
    Platform(PlatformAction),
    User(UserAction),
    Protocol(ServerEvent),
    Tick(TickKind),
    Command(CommandId),
}
```

Examples:

```rust
pub enum UserAction {
    SubmitPrompt,
    EditComposer(ComposerEdit),
    ScrollTranscript { delta_px: f32 },
    SelectSession(SessionId),
    CancelRun,
    ToggleActivityPanel,
    OpenCommandPalette,
}
```

Reducers should return effects rather than performing side effects directly:

```rust
pub enum ClientEffect {
    SendRequest(Request),
    StartDaemon,
    OpenExternalEditor(PathBuf),
    CopyToClipboard(String),
    ShowNotification(Notification),
    RequestRender,
}
```

This is the clean boundary that the current TUI mostly lacks.

## Transcript model

The TUI currently reduces history/events into `DisplayMessage` and then terminal lines. Desktop needs a richer block model.

Suggested model:

```rust
struct TranscriptState {
    blocks: Vec<TranscriptBlock>,
    block_index: HashMap<BlockId, usize>,
    streaming_block: Option<BlockId>,
    version: u64,
}

enum TranscriptBlock {
    User(UserBlock),
    Assistant(AssistantBlock),
    Tool(ToolBlock),
    System(SystemBlock),
    BackgroundTask(TaskBlock),
    Swarm(SwarmBlock),
    Usage(UsageBlock),
    Memory(MemoryBlock),
    Compaction(CompactionBlock),
}
```

This becomes the common semantic representation.

The TUI can continue rendering terminal lines from this model later. The desktop will render custom cards/rows from it.

### Desktop rendering path

```text
TranscriptBlock
  -> DesktopTimelineItem
    -> UI nodes
      -> layout boxes
        -> text layout runs
          -> display list
```

Do not use terminal wrapped lines as the desktop source of truth.

## Feature mapping from TUI to desktop

| TUI feature | Current TUI shape | Desktop architecture |
|---|---|---|
| Chat transcript | `DisplayMessage` + wrapped `Line`s | `TranscriptBlock` + virtualized timeline |
| Streaming assistant text | `streaming_text` + incremental markdown | append-aware block text cache |
| Tool calls | `ToolCall` display messages and streaming tool calls | tool cards with compact/expanded states |
| Batch progress | `BatchProgress` in status/message prep | activity item + inline timeline block |
| Composer | terminal input string/cursor | custom text input model, IME-aware later |
| Queued messages | app queue fields | composer/session queue state in client-core |
| Soft interrupts | protocol events and pending queue | visible interruption banner/queue chip |
| Header/status | `ui_header`, `info_widget` | top bar + status/activity regions |
| Side pane | pinned diff/content/diagram pane | inspector panel with tabs |
| Mermaid/diagrams | terminal/image pane | image/vector surface, later side inspector |
| Diffs | terminal inline/pinned/file modes | changed files panel, diff cards, later hunk UI |
| Session picker | modal overlay | command palette/session switcher modal |
| Login/account picker | terminal overlays | settings/account modal views |
| Commands | slash commands/key handlers | shared command registry + palette + menus |
| Copy selection | line/cell ranges | semantic block/text selection |
| Workspace map | TUI workspace widget | session/workspace sidebar, optional spatial mode |
| Debug overlays | visual debug/frame capture | performance HUD + UI tree/render inspector |
| Reconnect | remote loop state machine | protocol client state machine in client-core/app |
| Replay | replay mode | event-log replay harness for desktop UI tests |

## Desktop product module layout

`jcode-desktop` should be product composition, not renderer internals.

Suggested modules:

```text
crates/jcode-desktop/src/
  main.rs
  app.rs                    # top-level DesktopApp orchestration
  config.rs
  protocol_client.rs         # socket connection, read/write tasks
  daemon.rs                  # start/connect/find bundled daemon
  views/
    root.rs
    top_bar.rs
    session_sidebar.rs
    timeline.rs
    timeline_blocks.rs
    composer.rs
    activity_panel.rs
    workspace_panel.rs
    inspector_panel.rs
    command_palette.rs
    permission_modal.rs
    settings.rs
    debug_hud.rs
  reducers/
    platform_events.rs
    commands.rs
    view_actions.rs
  macos/
    bundle.rs                # build/package helpers if needed
    appkit_hooks.rs           # menus/lifecycle if winit is insufficient
```

## Custom UI crate layout

`jcode-desktop-ui` is the framework-like internal layer, but it is product-owned and small.

```text
crates/jcode-desktop-ui/src/
  lib.rs
  id.rs
  geometry.rs
  color.rs
  style.rs
  theme.rs
  input.rs
  focus.rs
  accessibility.rs            # semantic tree placeholder, not full impl initially
  tree.rs                     # retained node tree
  widget.rs                   # view builder traits/types
  layout/
    mod.rs
    flex.rs
    stack.rs
    split.rs
    scroll.rs
    virtual_list.rs
  text/
    mod.rs
    buffer.rs
    selection.rs
    shaping.rs
    cache.rs
  display_list.rs
  invalidation.rs
  animation.rs                # minimal timers only, no full animation system initially
  debug.rs
```

This crate should expose primitives such as:

```rust
pub enum UiNodeKind {
    Row,
    Column,
    Stack,
    SplitPane,
    Scroll,
    VirtualList,
    Text,
    TextInput,
    Button,
    CustomPaint,
}
```

But product views should mostly build specialized surfaces rather than generic widget soup.

## Renderer crate layout

`jcode-desktop-renderer` should know nothing about Jcode.

```text
crates/jcode-desktop-renderer/src/
  lib.rs
  gpu.rs
  surface.rs
  pipeline.rs
  primitives.rs
  text_renderer.rs
  glyph_atlas.rs
  image_atlas.rs
  clips.rs
  stats.rs
  screenshot.rs
```

Input:

```rust
struct DisplayList {
    commands: Vec<DrawCommand>,
}

enum DrawCommand {
    Rect(RectPaint),
    Border(BorderPaint),
    Text(TextPaint),
    Image(ImagePaint),
    ClipBegin(Rect),
    ClipEnd,
}
```

Output:

- frame rendered
- renderer stats
- optional screenshot/debug capture

The renderer should be testable with deterministic display lists and should support headless/golden rendering later if practical.

## Platform crate layout

Start with `winit`, but avoid spreading `winit` types through the product.

```text
crates/jcode-desktop-platform/src/
  lib.rs
  event.rs
  window.rs
  clipboard.rs
  menus.rs
  dialogs.rs
  appearance.rs
  shortcuts.rs
  macos.rs
  linux.rs
```

Normalize platform differences:

```rust
enum PlatformEvent {
    WindowResized { size: PhysicalSize, scale: f64 },
    ScaleFactorChanged { scale: f64 },
    RedrawRequested,
    Keyboard(KeyboardEvent),
    Pointer(PointerEvent),
    Scroll(ScrollEvent),
    FilesDropped(Vec<PathBuf>),
    AppearanceChanged(Appearance),
    AppShouldQuit,
}
```

Keyboard shortcuts should use platform semantic modifiers:

```rust
enum ShortcutModifier {
    Primary, // Cmd on macOS, Ctrl elsewhere
    Alt,
    Shift,
    Control,
    Command,
}
```

## Render/update loop

The TUI uses a redraw interval and `needs_redraw`. Desktop should keep the same spirit but be stricter.

```text
wait for platform/protocol/timer event
  -> normalize event
  -> reducer updates client-core/app state
  -> collect effects
  -> mark dirty UI nodes
  -> if render requested:
       layout dirty/visible nodes
       shape dirty text
       build display list
       submit wgpu frame
       publish debug stats
```

Rules:

- no continuous render loop when idle
- no full transcript re-layout on token append
- no unbounded visible node count
- protocol events may coalesce before rendering
- animations must explicitly schedule the next frame
- frame stats should be available before real feature integration is considered done

## Reuse path for existing TUI behavior

Do not stop all TUI work to extract everything first. Use an incremental route.

### Phase 1: desktop prototype independent of TUI internals

Build:

- desktop crates/modules
- fake transcript/activity data
- virtualized timeline
- debug HUD
- protocol-shaped fake events

Avoid depending on `src/tui`.

### Phase 2: protocol reuse

Use the existing server protocol:

- connect to `jcode serve`
- subscribe/resume session
- receive `ServerEvent`
- send `Request::Message`, `Request::Cancel`, etc.

Implement a desktop protocol reducer that mirrors the important behavior in `src/tui/app/remote/server_events.rs`, but writes to `ClientCore`/`TranscriptBlock`, not `DisplayMessage`.

### Phase 3: extract client-core

Once the desktop reducer shape is clear, extract shared pieces from TUI and desktop into `jcode-client-core`:

- transcript block model
- server event reducer
- command registry metadata
- activity model
- status model
- session list model
- permission model

At that point the TUI can gradually become another presentation of `jcode-client-core`, but it does not have to be converted all at once.

### Phase 4: feature parity

Add desktop versions of TUI features in priority order:

1. sessions, transcript, composer, send/cancel
2. tool cards and streaming output
3. activity panel and background tasks
4. command palette and core slash commands
5. permission prompts
6. session picker/resume/search
7. workspace/git/changed files
8. settings/login/account surfaces
9. diff/diagram inspector
10. debug/replay/profiling surfaces

## Desktop should be server-first

The TUI still supports local mode and remote/server mode. The desktop should start server-first.

Recommended desktop rule:

> Desktop always connects to a local Jcode server/daemon. It does not embed the agent runtime in-process.

Reasons:

- avoids UI freezes from runtime work
- keeps CLI/TUI/desktop as peers
- reuses reconnect/session lifecycle
- simpler crash isolation
- easier macOS bundle helper model
- avoids another local-mode runtime path

## Differences from the TUI model

### Scrolling

TUI scroll is line/cell based. Desktop scroll should be pixel based with fractional offsets.

```rust
struct ScrollState {
    offset_px: f32,
    velocity_px: f32,
    anchor: Option<ScrollAnchor>,
    auto_scroll: bool,
}
```

Virtualization should happen by pixel range and estimated/measured block heights.

### Text

TUI text is terminal spans and display widths. Desktop text should be shaped runs and glyph positions.

Desktop text caches should be keyed by:

- block ID
- content version/hash
- style
- available width
- font scale
- platform scale factor

### Selection

TUI selection is line/cell based. Desktop should use semantic selection:

- block ID
- text range within block
- optional structured copy target

### Layout

TUI layout is frame-sized terminal rects. Desktop should use a retained layout tree with dirty flags.

### Rendering caches

The TUI has several global caches. Desktop caches should be instance-owned and attributable:

```rust
struct RenderCaches {
    text: TextLayoutCache,
    glyphs: GlyphAtlas,
    images: ImageAtlas,
    timeline_measurements: MeasurementCache,
}
```

No process-global renderer state unless it is explicitly immutable/static.

## Testing strategy

Desktop should borrow the TUI's debug-first mentality, but use desktop-appropriate tests.

Required early tests:

- protocol reducer tests from `ServerEvent` sequences to `TranscriptBlock` state
- transcript virtualization tests with 100k fake blocks
- scroll anchor tests during streaming append
- layout tests for split panes and timeline rows
- text cache invalidation tests
- command registry tests
- display-list snapshot tests for stable fake UI states
- replay tests using captured protocol event logs

Avoid depending on GPU tests for basic correctness. Most UI behavior should be validated before `wgpu` submission.

## Implementation recommendation

Start by adding desktop code without touching the TUI too much:

```text
crates/jcode-desktop-ui       # pure-ish UI/layout model
crates/jcode-desktop-renderer # wgpu display-list renderer
crates/jcode-desktop          # fake-data app shell
```

Then connect to the server protocol.

Only after the desktop reducer/view model shape is proven should shared `jcode-client-core` extraction begin. This avoids prematurely extracting the wrong abstraction from the current TUI.

## Summary decision

The desktop app should be architected as a new custom presentation stack over shared client/runtime concepts, not as a ratatui port.

The TUI remains the feature reference. The server/protocol remains the runtime foundation. The new shared layer should be `client-core`, which owns surface-independent app behavior and view models. The desktop-specific code should focus on platform integration, retained UI, custom layout, text shaping, virtualization, and `wgpu` rendering.
