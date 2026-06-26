# Explorer: repo-distribution

How to LINK `1jehuang/jcode` and `1jehuang/scrollwm` at the project /
distribution level, the version-compat + feature-detection handshake, the
Homebrew tap relationship, the cross-repo docs plan, and the scripted
marquee demo. Both repos are owned by the same user but ship on different
toolchains (Rust/cargo vs Swift/SwiftPM) and cadences.

## TL;DR recommendation

- **Repo linking: keep two separate repos (option a) + a small versioned
  integration contract.** Do NOT submodule. The integration contract is a
  short spec file that lives canonically in `scrollwm` (it owns the wire
  format) and is mirrored/linked from `jcode`. The runtime coupling is a
  **capability handshake over the existing `scrollwm status` JSON**, not a
  build-time dependency.
- **Distribution: the linking already exists** - the `1jehuang/homebrew-jstack`
  tap has a `jstack` bundle cask (`depends_on cask: scrollwm`). Lean into it:
  jcode onboarding suggests `brew install --cask 1jehuang/jstack/scrollwm` (or
  the web-install one-liner), scrollwm's docs/Config reference jcode.
- **Coupling is one-directional at the wire level, advertised both ways:**
  jcode is the *client* (drives scrollwm via `scrollwm <verb>` / the control
  socket); scrollwm stays jcode-agnostic but ships a commented jcode launcher
  and a `jcode`-aware spawn hint. Each ships independently; neither build
  depends on the other.

---

## 1. The four repo-linking options, scored

| Option | What it is | Pros | Cons | Verdict |
|---|---|---|---|---|
| (a) Separate repos + integration contract | Two repos, a versioned `INTEGRATION.md`/`integration.json` spec defining the socket verbs + status JSON + capability flags | Independent release cadence (Rust vs Swift CI are totally different); no cross-language build coupling; each repo stays buildable in isolation; mirrors how they already ship | Contract can drift if not tested; needs a cheap conformance check | **CHOSEN** |
| (b) git submodule | jcode vendors scrollwm (or vice versa) as a submodule | Single checkout; pinned SHA | Forces a Swift toolchain into jcode CI for no benefit (jcode never *builds* scrollwm); painful submodule UX; couples release trains; the runtime artifact is a `.app` + a CLI, not source | Reject |
| (c) Thin shared "integration" doc in one repo | A spec file in one repo only | Low cost | Where it lives becomes a bikeshed; the *other* repo has no anchor | Subsumed by (a): the contract is exactly this, but **dual-anchored** |
| (d) Homebrew tap relationship | Casks advertise/depend on each other | Real install-time linking; already half-built (`jstack` cask) | Only covers *install*, not the runtime handshake | **Adopt alongside (a)** |

**Why (a)+(d), concretely:** jcode is Rust shipping via GitHub Releases +
`scripts/quick-release.sh` + CI updating `1jehuang/homebrew-jcode` and AUR
(`RELEASING.md`). scrollwm is a Swift `.app` shipping via
`scripts/web-install.sh`, `Casks/scrollwm.rb`, and an in-app self-updater
(`Updater.swift`, `UpdateCoordinator.swift`, `SemVer.swift`). These are
fundamentally different release pipelines. A submodule would drag the Swift
SDK / codesigning world into jcode's `cargo`/osxcross pipeline for zero gain,
because **jcode never compiles scrollwm** - it talks to a running app over a
Unix socket. So the only thing that must be shared is the *wire contract*,
which is a doc + a tiny JSON, not code.

---

## 2. Version compatibility + feature detection scheme

### 2.1 The handshake transport already exists

scrollwm exposes a Unix socket at
`~/Library/Application Support/ScrollWM/control.sock`
(override `SCROLLWM_CONTROL_SOCK`), driven by
`ControlServer.swift` -> `ControlCommands.handleControlCommand`
(`Sources/WindowLab/ControlCommands.swift`). The `scrollwm` CLI shim
(`ControlCLI.swift`) connects, sends one line, prints the reply, and exits with
a meaningful code (`0` ok / `2` `error:` / `3` not-running). **This is the
handshake channel.** jcode should detect + drive scrollwm by shelling out to
`scrollwm status` (or connecting to the socket directly), exactly like the
existing setup-hint pattern in `jcode-setup-hints`.

### 2.2 Gap to close (the ONE production change this area needs)

`controlStatusJSON()` in `ControlCommands.swift` currently emits:

```json
{ "managing": false, "focusMode": "...", "windowCount": 0, ... }
```

It has **no version and no capability list**. Add three fields so jcode can do
feature detection without sniffing the app bundle:

```jsonc
{
  "managing": true,
  "focusMode": "follow",
  "windowCount": 3,
  // --- new: integration handshake ---
  "version": "0.2.0",            // CFBundleShortVersionString (SemVer.swift parses it)
  "protocol": 1,                 // integer control-protocol revision, bumped on breaking verb/JSON change
  "capabilities": [             // feature flags jcode can branch on
    "arrange", "focus", "move", "workspace", "width",
    "display", "focus-mode", "spawn-adopt"
  ]
}
```

`version` is already available app-side via `Bundle.main
.infoDictionary["CFBundleShortVersionString"]` (see
`CodeSigning.swift:104-106` and `Updater.swift:28-29`). `protocol` is a new
monotonically-increasing integer constant living next to the verb switch.
`capabilities` is derived from the verb set so it never drifts from reality.

**Why protocol + capabilities, not just version:** jcode shouldn't hardcode
"scrollwm >= 0.2.0 has feature X". It should test `capabilities.contains("X")`.
The `protocol` integer is the coarse compatibility gate ("do we even speak the
same language"); `capabilities` is fine-grained feature detection. This is the
standard handshake-not-version-pinning pattern and lets either side ship
independently.

### 2.3 jcode-side detection (mirror the setup-hints nudge pattern)

Add a `jcode-scrollwm` detection helper (new tiny crate or a module under
`jcode-setup-hints`, which already owns `~/.jcode/setup_hints.json` and the
"nudge cap" pattern in `macos_launcher.rs`). Detection ladder:

1. `which scrollwm` present? (installed CLI shim) -> `Installed`.
2. `scrollwm status` exits 0 with parseable JSON -> `Running`, capture
   `{version, protocol, capabilities}`.
3. exit 3 / "isn't running" -> `InstalledNotRunning`.
4. not found -> `NotInstalled` (this is what onboarding offers to fix).

Cache the result (with a short TTL) in `setup_hints.json` so jcode doesn't
shell out on every spawn. Gate *driving* features on `capabilities`; gate the
whole integration on `protocol >= JCODE_MIN_SCROLLWM_PROTOCOL` (a jcode
constant). On `protocol` newer than jcode knows, degrade gracefully: use only
the verbs jcode understands, never error.

### 2.4 Compatibility policy (documented in the contract)

- `protocol` bumps ONLY on a breaking change to an existing verb's args or the
  status JSON shape. Adding a new verb or a new capability flag is **non-breaking**
  (bump nothing; new flag appears in `capabilities`).
- jcode declares `JCODE_MIN_SCROLLWM_PROTOCOL` and `JCODE_KNOWN_SCROLLWM_PROTOCOL`.
  If `status.protocol < min` -> treat as "incompatible, suggest `brew upgrade`".
  If `min <= status.protocol` -> use `capabilities` for everything else.
- scrollwm guarantees backward-compatible replies within a `protocol` rev.

---

## 3. Homebrew / tap relationship

### 3.1 Current state (already linked!)

`/Users/jeremy/homebrew-jstack` (`git@github.com:1jehuang/homebrew-jstack.git`)
already ships three casks:

- `jcode.rb` - CLI only, `conflicts_with cask: jstack`.
- `scrollwm.rb` - the `.app` + `scrollwm` CLI shim, `depends_on macos: :sonoma`.
- `jstack.rb` - **the bundle**: installs the jcode binary AND
  `depends_on cask: "1jehuang/jstack/scrollwm"`, `conflicts_with jcode`.

`scripts/update.sh` regenerates all three from each repo's latest GitHub
release + `SHA256SUMS`. So `brew install --cask 1jehuang/jstack/jstack` already
installs both in one shot. Note jcode *also* publishes to a separate
`1jehuang/homebrew-jcode` tap from its release CI (`RELEASING.md`,
`release.yml:333`) - that one is jcode-only and unaware of scrollwm.

### 3.2 Recommended tap topology

Keep the split, make the bundle the front door:

```
1jehuang/homebrew-jstack  (the integration tap, hand/CI-maintained)
  +- jstack   -> jcode + scrollwm bundle      <- marketed entry point
  +- jcode    -> CLI only
  +- scrollwm -> app only

1jehuang/homebrew-jcode   (jcode's own CI-updated tap, unchanged)
  +- jcode    -> CLI only, scrollwm-agnostic
```

Concrete tap improvements (small, low-risk):

1. **`scrollwm.rb` -> `jcode.rb` cross-suggest via caveats.** scrollwm's cask
   `caveats` should mention: "Pairs with jcode: `brew install --cask
   1jehuang/jstack/jcode` - jcode can auto-tile its agent windows in ScrollWM."
2. **`jcode.rb` caveats already say "Run `jcode`".** Add a one-liner: "Tip:
   install ScrollWM (`brew install --cask 1jehuang/jstack/scrollwm`) to let
   `jcode` tile headed swarm agents."
3. **Wire scrollwm into jcode's release CI tap optionally**: jcode's CI updates
   `homebrew-jcode`. Have a small follow-on step (or the existing
   `homebrew-jstack/scripts/update.sh` on a schedule) keep `jstack.rb` pinned to
   the latest jcode release so the bundle never lags. Today `update.sh` is
   manual; a nightly GitHub Action in the tap repo calling it closes the loop
   without coupling either product's CI.

The runtime handshake (section 2) means the casks do NOT need version-locked
`depends_on` between jcode and scrollwm - jcode feature-detects at runtime, so
mismatched cask versions degrade gracefully instead of failing to install.

---

## 4. Cross-repo docs plan

Dual-anchored contract, each side links the other. Canonical wire spec lives in
scrollwm (it owns the socket).

```
scrollwm/docs/INTEGRATION.md          <- CANONICAL contract:
    - socket path + env override
    - every verb + args + reply shape
    - the status JSON schema incl. version/protocol/capabilities
    - protocol/compat policy
    - "jcode is a first-class client" section + link to jcode docs

jcode/docs/scrollwm-integration/CONTRACT.md  <- thin pointer:
    - "ScrollWM owns the wire contract: <scrollwm link>"
    - jcode's constants: JCODE_MIN/KNOWN_SCROLLWM_PROTOCOL
    - the detection ladder + which capabilities jcode uses
    - install/onboarding UX cross-links
```

Plus light touches:

- **scrollwm `README.md`**: a "Use with jcode" section (tile your AI agents).
- **jcode `README.md`**: a "Window management (macOS): ScrollWM" section with
  the `brew install --cask 1jehuang/jstack/scrollwm` line.
- **homebrew-jstack `README.md`** already does the bundle pitch well; add a
  one-line "How they integrate" pointer to `scrollwm/docs/INTEGRATION.md`.
- **Conformance guard:** a tiny test in scrollwm asserting `controlStatusJSON()`
  emits `version`/`protocol`/`capabilities` and that `capabilities` matches the
  live verb set, so the doc/code never drift (lands as a `*Tests.swift` per the
  scrollwm swarm convention).

---

## 5. The marquee end-to-end demo (scripted)

Goal flow from the brief: **install jcode -> onboarding offers ScrollWM ->
spawn a headed swarm -> ScrollWM tiles the agents.**

### 5.1 The narrative beats

1. `brew install --cask 1jehuang/jstack/jstack`  (or `curl | bash` web-install,
   or `jcode` already installed).
2. First `jcode` run -> onboarding. After login/import phases
   (`OnboardingPhase` in `jcode-tui/.../onboarding_flow.rs`), a NEW opt-in row:
   "Install ScrollWM to auto-tile your agent windows? [Yes/No]" rendered like
   the existing Yes/No decision rows (`ui_onboarding.rs`,
   `draw_onboarding_welcome`). Yes -> run the scrollwm web-install one-liner /
   `brew install --cask`, then open System Settings for the single
   Accessibility grant (scrollwm handles the rest; it auto-continues on grant
   per `web-install.sh` next-steps).
3. User asks jcode to do work; jcode spawns a **headed** swarm with
   `swarm_spawn_mode = visible` (`SwarmSpawnMode::Visible`,
   `jcode-config-types`), each agent a new Ghostty window via
   `spawn_visible_session_window` -> `session_launch` ->
   `terminal_launch::spawn_command_in_new_terminal` (`build_spawn_command`,
   `open -na Ghostty ...`).
4. jcode, having detected scrollwm (section 2.3), drives it: after spawning the
   N agent windows it calls `scrollwm arrange` once, then `scrollwm focus <n>`
   to spotlight the active agent. ScrollWM tiles them into strip columns /
   workspaces. (This is the actual integration the other explorers design; this
   doc just scripts/demos it.)

### 5.2 Reproducible demo script (throwaway, lives in this doc)

A safe, sandbox-respecting capture script. **Obeys the scrollwm golden rule:**
it uses ScrollWM's own `sandbox` disposable windows for any pre-flight and only
arranges jcode's freshly-spawned agent windows, never the user's real windows.

```bash
#!/usr/bin/env bash
# docs/scrollwm-integration/demo/run_demo.sh  (throwaway capture harness)
set -euo pipefail

# 0. Preconditions ----------------------------------------------------------
command -v scrollwm >/dev/null || { echo "install scrollwm first"; exit 1; }
command -v jcode    >/dev/null || { echo "install jcode first"; exit 1; }

# 1. Handshake / feature detection (the section-2 contract in action) -------
status="$(scrollwm status 2>/dev/null || echo '{}')"
echo "ScrollWM status: $status"
# (Once version/protocol/capabilities land, branch on them here.)

# 2. Make sure ScrollWM is managing an EMPTY strip (no real windows) --------
#    Launch it dormant; do NOT 'arrange' the real desktop.
open -a ScrollWM || true
sleep 1

# 3. Spawn a headed jcode swarm (visible Ghostty windows) -------------------
#    Configure visible spawns, then ask jcode to fan out work.
jcode --set agents.swarm_spawn_mode=visible \
      -p 'Spawn 3 headed swarm agents that each summarize one file in ./src, \
          then arrange them with ScrollWM.'

# 4. Tile + spotlight: jcode does this internally once the integration ships;
#    here we show the verbs explicitly for the demo voiceover.
scrollwm arrange              # tile the agent windows into the strip
scrollwm focus 1              # spotlight agent #1
sleep 1; scrollwm focus next  # walk across agents for the camera
sleep 1; scrollwm focus next

# 5. Teardown: release restores every window to where it was ----------------
scrollwm release
```

For an automated screen capture, wrap with macOS `screencapture -v` (or reuse
the repo's `scripts/record_demo.sh` / `capture_demo.sh` patterns, which are
currently niri/Linux-specific and would get a macOS sibling). The existing
`~/Desktop/scrollwm + jcode demo.mov` (23 MB, recorded 2026-06-25) is the
reference look; this script reproduces it deterministically.

### 5.3 What makes the demo "marquee"

- Single command to install both (`brew install --cask 1jehuang/jstack/jstack`).
- One permission (Accessibility) the whole way.
- The "wow": you tell jcode to do parallel work, three terminals fly in, and
  ScrollWM instantly tiles them into a scrolling strip you can scrub across with
  `cmd+h`/`cmd+l` while agents work. Then `release` and your desktop is exactly
  as it was - the safety story is part of the pitch.

---

## 6. Risks, edge cases, alternatives

- **Drift between doc and socket:** mitigated by the conformance test in 5/§4
  and by deriving `capabilities` from the live verb set, not a hand-list.
- **scrollwm not running when jcode wants to tile:** `scrollwm arrange` already
  auto-launches for `arrange`/`toggle` (`ControlCLI.swift launchVerbs`); jcode
  should rely on that and tolerate exit 3.
- **Golden rule violations in demos/tests:** never `scrollwm arrange` in a test
  against the live desktop; use scrollwm `sandbox` windows or only
  jcode-spawned windows. The demo script above is written to honor this.
- **Cask version skew:** acceptable by design - runtime handshake degrades; do
  NOT add tight `depends_on` version pins between jcode and scrollwm casks.
- **Two jcode taps (`homebrew-jcode` vs `homebrew-jstack`):** keep both;
  document that `jstack`/`jcode` casks conflict (already encoded via
  `conflicts_with`). The README already calls this out.
- **Alternative considered - jcode bundles a scrollwm client lib:** unnecessary;
  the socket + `scrollwm` CLI is the public API. A thin Rust wrapper around
  `scrollwm <verb>` (or a raw `UnixStream`) is all jcode needs.

---

## 7. Minimal first PR for this area

Two tiny, independently-shippable PRs (one per repo) + a docs commit:

1. **scrollwm PR** (`ControlCommands.swift`): add `version`, `protocol` (=1),
   and `capabilities` to `controlStatusJSON()`, plus a `ControlStatusTests.swift`
   asserting the fields exist and `capabilities` matches the verb set. Create
   `scrollwm/docs/INTEGRATION.md` (canonical contract). No behavior change to
   window management. ~40 lines + test.
2. **jcode PR** (docs + constants only, no production wiring yet): add
   `jcode/docs/scrollwm-integration/CONTRACT.md` pointing at the scrollwm spec,
   define `JCODE_MIN_SCROLLWM_PROTOCOL`/`JCODE_KNOWN_SCROLLWM_PROTOCOL`
   constants (in the future `jcode-scrollwm` module), and add a README "Window
   management: ScrollWM" section with the brew line.
3. **homebrew-jstack PR**: add cross-suggest `caveats` to `jcode.rb` and
   `scrollwm.rb`, and a nightly Action calling `scripts/update.sh` so `jstack`
   tracks the latest jcode release automatically.

These three land in any order, each repo stays green, and together they
establish the version handshake + the advertised-but-decoupled relationship the
later integration PRs (spawn/tile wiring, onboarding opt-in row) build on.
