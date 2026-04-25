# Desktop First Prototype Target

Status: Proposed
Updated: 2026-04-25

The first implementation step for Jcode Desktop should be **Phase 0: a fullscreen blank white canvas**.

Do not start with:

- fake workspace surfaces
- real server integration
- a full editor
- any browser work
- settings/auth flows
- packaging
- perfect text rendering

Start by proving the absolute foundation:

> a native fullscreen window with a custom GPU-rendered white canvas.

## Phase 0 visual target

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                              blank white canvas                              │
│                                                                              │
│                                                                              │
│                                                                              │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

## What Phase 0 must prove

1. A native window opens on Linux.
2. The window enters fullscreen or borderless fullscreen mode.
3. The app creates a GPU surface.
4. The app clears the surface to white.
5. The app handles resize/scale-factor changes without crashing.
6. The app exits cleanly with `Esc` or close-window.
7. The app uses an on-demand event loop rather than a busy render loop.
8. The app can be built and run independently from the TUI.

## Why this comes before the spatial workspace

A blank canvas is intentionally tiny. It validates the platform/rendering foundation before adding product complexity.

It answers:

- Can we create the desktop crate cleanly?
- Does `winit` work as the initial platform shell?
- Does `wgpu` initialize on the Linux dev machine?
- Can we render a frame without a web view or UI framework?
- Can fullscreen behavior be tested early?

## Linux desktop entry

The repository includes an install-oriented desktop entry at:

```text
packaging/linux/jcode-desktop.desktop
```

It expects a `jcode-desktop` binary to be available on `PATH`. For local testing after installing or copying the binary somewhere your desktop launcher can execute, copy the entry to your user applications directory:

```bash
mkdir -p ~/.local/share/applications
cp packaging/linux/jcode-desktop.desktop ~/.local/share/applications/
update-desktop-database ~/.local/share/applications 2>/dev/null || true
```

## Phase 1 target after this

Once Phase 0 works, the next prototype is the fake-data spatial workspace:

```text
┌────────────────────────────────────────────────────────────────────────────────────┐
│ Jcode Workspace: jcode repo      leader: Space     mode: NAV     4 sessions   2 run │
├──────────────┬────────────────────────────┬────────────────────────────┬────────────┤
│ sessions     │ ● fox / coordinator        │   wolf / impl              │ activity   │
│              │                            │                            │            │
│ ● fox        │ user: make desktop app     │ user: inspect tui arch     │ build      │
│ ○ wolf       │                            │                            │ cargo test │
│ ○ owl        │ assistant: plan surfaces   │ assistant: found protocol  │ 42%        │
│ ○ bear       │                            │                            │            │
│              │ tool: read docs            │ tool: grep ServerEvent     │ pending    │
│ files        │ tool: edit architecture    │ tool: summarize tui        │ approval   │
│ diffs        │                            │                            │            │
│ debug        │ composer inactive          │ composer inactive          │            │
├──────────────┴────────────────────────────┴────────────────────────────┴────────────┤
│ Space h/j/k/l focus  ·  Space H/J/K/L move  ·  Space n new session  ·  Space / cmd │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Phase 1 proves the actual product bet:

- multiple visible agent sessions
- Niri-like spatial layout
- leader + `h/j/k/l` navigation
- move/close/zoom surfaces
- independent fake transcripts
- activity surface
- custom rendering performance
- near-zero idle CPU

## Initial Phase 1 surface kinds

```rust
enum SurfaceKind {
    AgentSession,
    Activity,
    WorkspaceFiles,
    Diff,
    Debug,
}
```

No browser preplanning. No full editor yet.

## Phase 1 success bar

The fake workspace prototype is successful when a user can launch it, see multiple fake sessions, move between them with leader+`h/j/k/l`, create/move/close/zoom surfaces, and confirm the app remains smooth and idle-efficient.
