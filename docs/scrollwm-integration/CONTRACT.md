# jcode <-> ScrollWM integration contract (jcode side)

The **canonical** wire contract lives in the ScrollWM repo:
<https://github.com/1jehuang/scrollwm/blob/main/docs/INTEGRATION.md>

This file is the jcode-side pointer + the constants/decisions jcode commits to.

## What jcode ships

- **`crates/jcode-scrollwm`** - a small, blocking, best-effort Rust client for
  ScrollWM's Unix control socket (the Rust counterpart to ScrollWM's Swift
  `ControlClient.send`). Detects ScrollWM (`is_running`, `hello`), reads the
  strip (`status`), and drives it (`arrange`, `focus_index`, `focus_title`).
- **`agents.scrollwm` config** (in `jcode-config-types`) - the opt-in switch for
  using ScrollWM when spawning headed swarm agents:
  - `enabled` (default `false`)
  - `focus_active` (default `true`) - focus the just-spawned agent's column
  - `arrange_on_spawn` (default `false`) - adopt the whole Space (use with care)
  - env overrides: `JCODE_SCROLLWM`, `JCODE_SCROLLWM_FOCUS_ACTIVE`,
    `JCODE_SCROLLWM_ARRANGE_ON_SPAWN`
- **Onboarding opt-in** - a one-time "Set up ScrollWM?" step installs ScrollWM
  via its web installer (`crates/jcode-tui` onboarding flow). macOS only.

## Compatibility stance

- jcode gates the integration on `ScrollWm::is_running()` (ping), and treats a
  missing/old ScrollWM as simply absent: every control call is fire-and-log, so
  ScrollWM never affects jcode functionality.
- jcode reads the `version`/`protocol`/`capabilities` handshake (ScrollWM
  protocol >= 1) but currently only uses verbs present since protocol 0
  (`status`, `arrange`, `focus`), so it works against any ScrollWM that exposes
  the socket. New ScrollWM verbs (`focus-title`, `arrange-pids`, ...) will be
  gated on `capabilities.contains(...)` as jcode adopts them.

## Where the runtime coupling lives in jcode

- Detection + drive: `jcode-scrollwm` (client) +
  `jcode-app-core/src/server/comm_session.rs::maybe_reconcile_scrollwm_after_spawn`
  (the hook after a headed agent spawns).
- Config: `jcode-config-types::ScrollwmIntegrationConfig`,
  env overrides in `jcode-base/src/config/env_overrides.rs`.
- Onboarding: `jcode-tui/src/tui/app/onboarding_flow*.rs`.

## Distribution link

The two products ship independently but are bundled in the
`1jehuang/homebrew-jstack` tap: the `jstack` cask installs jcode and
`depends_on cask: scrollwm`. No build-time dependency exists between the repos;
the only shared surface is the wire contract above.
