# ScrollWM x jcode integration - explorer brief

You are one of several parallel **headed** explorer agents. Goal: explore how to
integrate **ScrollWM** (a Swift macOS scrolling window manager) with **jcode**
(this Rust TUI agent), and how to link the two repos. Each explorer owns ONE
area, investigates deeply in the real code, and writes a findings doc.

## The two repos (read-only unless told otherwise)
- ScrollWM: `/Users/jeremy/scrollwm` (Swift, SwiftPM, menu-bar app).
  - Control plane: `Sources/WindowLab/ControlServer.swift` (Unix socket),
    `ControlCommands.swift` (verbs: ping/status/arrange/release/toggle/focus/
    move/workspace/width/close/display/focus-mode/reload/update/quit),
    `ControlCLI.swift` (`scrollwm <verb>` shim), `Config.swift` (keybinds, and a
    commented-out `ctrl+opt+j` jcode launcher + `spawn` map).
  - Socket path: `~/Library/Application Support/ScrollWM/control.sock`, override
    via `SCROLLWM_CONTROL_SOCK`. status returns JSON.
  - Install: `scripts/web-install.sh` (curl|bash), Homebrew cask
    `1jehuang/scrollwm/scrollwm`, `scripts/install.sh` (build from source).
    One permission: Accessibility. git remote: github.com/1jehuang/scrollwm.
- jcode: `/Users/jeremy/jcode` (Rust). Relevant crates:
  - `jcode-swarm-core` (multi-agent swarm; `is_headless`), swarm spawn modes in
    `jcode-config-types` `SwarmSpawnMode` (visible/headless/inline/auto),
    spawn wiring in `jcode-app-core/src/server/client_lifecycle.rs`.
  - `jcode-terminal-launch` (`open -na Ghostty ...` visible terminal spawn),
    `jcode-setup-hints` (startup nudges, `~/.jcode/setup_hints.json`,
    macOS terminal/hotkey setup, the nudge cap pattern).
  - Onboarding: `jcode-tui/src/tui/app/onboarding_flow.rs` (state machine,
    `OnboardingPhase`), `onboarding_flow_control.rs` (transitions),
    `ui_onboarding.rs` (render of decision rows with Yes/No selectors),
    `jcode-app-core/src/external_auth.rs`
    (`pending_external_auth_review_candidates` = what is searched/found).
  - There is a `~/Desktop/scrollwm + jcode demo.mov` already.

## The product goals (from the user)
1. Integrate jcode with ScrollWM: when jcode spawns **headed** swarm agents,
   ScrollWM should arrange them nicely (strip columns / workspaces), and jcode
   should be able to drive ScrollWM (focus the active agent, etc).
2. Onboarding opt-in: during jcode onboarding, offer to install + set up
   ScrollWM (Accessibility permission) alongside jcode.
3. Onboarding "not found" transparency: on any onboarding decision row (e.g. the
   external-login import walkthrough), ALSO show the list of things we searched
   for but did NOT find, beneath the row. Add scrolling if the list overflows.

## Your deliverable
Write `docs/scrollwm-integration/explorers/<area>.md` in the jcode repo with:
- Concrete integration design for your area (with file/function anchors).
- Exact code-change sketch (what to add/modify, where), API surface, data flow.
- Risks, edge cases, alternatives, and a recommended approach.
- A short "minimal first PR" you'd ship for your area.
Keep it tight and concrete. Cite real symbols/paths. Do NOT make production code
changes; this is exploration. You MAY build small throwaway probes.

## Rules
- macOS live machine. ScrollWM safety contract: NEVER arrange the user's real
  windows in a test; only sandbox/disposable windows. Read its docs first.
- Report back to the coordinator when done (swarm report), with the path to your
  doc and a 5-line summary.
