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
2. The app supports fullscreen or borderless fullscreen mode via `--fullscreen`.
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

Once Phase 0 works, the next prototype is the fake-data spatial workspace. The first slice should prove the core Niri/Vim-style interaction model before real sessions or text rendering:

```text
Navigation mode:
  h/l          focus columns within the current workspace
  j/k          move to the workspace below/above
  H/L          move the focused column left/right
  J/K          move the focused column to the workspace below/above
  n            create a fake session surface
  Ctrl+;       create a fake session surface
  Ctrl+?       open/focus hotkey help
  Ctrl+1       prefer 25%-screen-wide panels
  Ctrl+2       prefer 50%-screen-wide panels
  Ctrl+3       prefer 75%-screen-wide panels
  Ctrl+4       prefer 100%-screen-wide panels
  x            close the focused surface
  z            zoom/unzoom the focused surface
  i or Enter   enter insert mode
  Esc          quit the prototype

Insert mode:
  typing       captured as draft input
  Esc          return to navigation mode
```

The initial renderer may use only primitive colored/rounded primitives and the native window title for detailed mode/status text. Full text rendering can follow after the workspace behavior feels right. The visual direction should put the color in a soft static blue/lavender/mint gradient background, with mostly transparent dark/neutral glass-like panels on top, muted status colors, a very thin gray focus ring, and visible but subdued unfocused borders. A compact status bar should sit at the top, not the bottom. Individual panels should not have their own top header bars until real text/chrome is useful. Panels should fill most of the available space with only narrow gutters and slightly rounded corners. Panel count should adapt to both the current desktop app window size and the user-selected preferred panel size: `Ctrl+1` prefers 25%-screen-wide panels, `Ctrl+2` prefers 50%, `Ctrl+3` prefers 75%, and `Ctrl+4` prefers 100%. A fullscreen app with `Ctrl+1` can show four columns, while fullscreen with `Ctrl+4` shows one column. A 25%-screen-width app window shows one column regardless of preset because only one preferred quarter-screen panel fits. The layout direction is Niri-like: each workspace is a vertical lane containing a horizontally scrollable strip of full-height columns. Columns should never be stacked within the same workspace. The top status bar should include a left-side active workspace number, a centered flattened Waybar-like preview strip, and eventually right-side mode/session text once text rendering lands. In the preview strip, nearby workspaces are shown as compact horizontal groups, each panel is a colored tick/block, inactive workspaces are dimmed, the active workspace group is highlighted, the focused panel is strongest, and the visible horizontal viewport is outlined. Smooth viewport/camera animations should make focus, workspace, spawn/close, and panel-size changes legible instead of teleporting instantly.

The target shape is:

```text
┌────────────────────────────────────────────────────────────────────────────────────┐
│ workspace 0 · NAV                                                                  │
├────────────────────┬────────────────────┬────────────────────┬────────────────────┤
│ ● fox/coordinator  │   wolf/impl        │   owl/review       │   activity         │
│                    │                    │                    │                    │
│ full-height column │ full-height column │ full-height column │ full-height column │
│                    │                    │                    │                    │
│                    │                    │                    │                    │
│                    │                    │                    │                    │
├────────────────────┴────────────────────┴────────────────────┴────────────────────┤
│ h/l columns · j/k workspaces · Ctrl+; new · Ctrl+? help · n new · z zoom           │
└────────────────────────────────────────────────────────────────────────────────────┘
```

Phase 1 proves the actual product bet:

- multiple visible agent sessions
- Niri-like spatial layout
- `h/l` column navigation and `j/k` workspace navigation
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
