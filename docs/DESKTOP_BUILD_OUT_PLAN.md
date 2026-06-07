# Jcode Desktop Build-Out Plan

Status: Active planning note
Updated: 2026-05-25

## Current read

Jcode Desktop has moved past the original blank-canvas prototype. The strongest current thread is a single-session desktop surface with a local GitHub issue browser/cache and background `gh` sync. Recent work added:

- issue-browser renderer extraction
- issue-browser interactions
- local GitHub issue cache
- background issue sync UI state
- tests for issue navigation, investigation prompt injection, cache ranking, and sync failure behavior

The product docs still point at the long-term goal: a keyboard-driven, Niri-like local AI development superapp. The right next move is not to jump directly to a full IDE or complete workspace manager. Build outward from the proven single-session surface into issue-driven agent workflow and reusable surface primitives.

## Recommended product sequence

### 1. Finish the GitHub issue workflow until it is genuinely useful

This is the most valuable near-term build-out because it connects desktop UI, repo context, agent launch, and an actionable developer workflow.

Build next:

1. Issue sync polish
   - visible auth/install guidance when `gh` is missing or unauthenticated
   - sync timestamp and cache path surfaced in the browser
   - manual refresh shortcut help in the UI
   - bounded background sync status that cannot look stuck
2. Issue filtering and triage
   - filter by label/state/text
   - quick priority override persisted in `local_overrides`
   - hide/done/local-dismiss issue affordance
3. Issue-to-agent workflow
   - start a new session from selected issue
   - inject structured issue context, comments, labels, and repo facts
   - optionally spawn an implementation agent and a review/check agent
4. Issue activity loop
   - show running sessions associated with issue numbers
   - show changed files/test status once an agent acts
   - jump from issue browser to the relevant session

Verification:

- `cargo test -p jcode-desktop issue --no-default-features`
- fake `gh` runner tests for auth/missing CLI/comment failures
- UI snapshot/layout tests for empty, syncing, error, and narrow-width states

### 2. Extract generic surface/workspace primitives from single-session state

Do this after the issue workflow has one concrete non-chat side surface. Avoid speculative architecture, but create the minimum model needed to compose session plus issue/activity surfaces.

Build next:

1. `SurfaceId`, `SurfaceKind`, `SurfaceState`
2. one lane with horizontal columns
3. focus movement left/right
4. zoom/unzoom focused surface
5. persistent layout per repo/workspace
6. surface command routing independent of renderer

Start with these surface kinds:

- `AgentSession`
- `GitHubIssues`
- `Activity`
- `DiffSummary` or `ChangedFiles`

Verification:

- pure state-machine tests for focus, move, close, zoom, and persistence
- renderer tests proving existing single-session surface still composes without regressions
- keyboard-routing tests separating navigation mode from text-entry mode

### 3. Build an activity/mission-control surface

The desktop app should excel at supervising autonomous work. Activity should become a first-class surface rather than a small status panel.

Build next:

1. stream current session status, background tasks, tool runs, and build/test jobs
2. filter by workspace/session/issue/tool type
3. select activity item to jump to session/tool output
4. show blocked permission prompts prominently
5. show recently completed validation results

Verification:

- synthetic event batch tests for high-volume updates
- no busy render loop while idle
- activity jump tests into session transcript/tool cards

### 4. Add Level-1 files/diff surfaces, not a full editor

Desktop needs review affordances before it needs editing.

Build next:

1. changed-files list
2. read-only file preview
3. read-only unified diff preview
4. open path/diff in external editor
5. copy path/snippet
6. link changed files back to sessions/issues

Verification:

- large diff/file virtualization tests
- binary/large-file fallbacks
- external-open command tests behind safe mocks

### 5. Command palette and shared command registry

Once there are multiple surfaces, the command palette should be the universal router.

Build next:

1. command registry for desktop commands
2. fuzzy palette over commands, sessions, issues, files, and surfaces
3. keyboard shortcut discovery from the same registry
4. palette actions with safe confirmation classification

Verification:

- registry uniqueness tests
- fuzzy ranking tests
- all visible shortcuts generated from registered commands

## Explicit non-goals for the next tranche

Do not build these yet:

- full code editor
- plugin/extension system
- general desktop window manager behavior
- elaborate multi-window support
- large settings/auth UI beyond what unblocks issue sync and model/session workflow
- webview/Electron/Tauri shell

## Next concrete implementation slice

The best immediate slice is:

> Make `/issues` a robust issue-to-agent launcher.

Definition of done:

1. Opening `/issues` loads cache immediately and starts a background sync when appropriate.
2. Empty/error states explain how to install/authenticate `gh`.
3. Pressing the investigate/start key opens or spawns a real session with full structured issue context.
4. The issue browser shows sync status, last sync, comment fetch failures, and selected issue context clearly.
5. Tests cover success, missing `gh`, auth failure, sync partial failure, empty cache, and narrow layout.

This slice is small enough to ship, but it moves the desktop app toward its core product identity: issue-driven agent mission control.
