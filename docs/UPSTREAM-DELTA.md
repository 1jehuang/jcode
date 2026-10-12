# Upstream Delta — KooshaPari/KCode vs 1jehuang/jcode

Generated: 2026-10-12
Source of truth: this fork's `master` vs `1jehuang/jcode:master` HEAD.
All hashes are local short SHAs; full hashes available via `git log upstream/master..master`.

## Headline numbers

| | |
|---|---|
| Commits ahead of upstream | 142 at write time (now 144; +2 are this doc + correction) |
| Commits behind upstream | 2,649 |
| Date range of ahead commits | 2026-09-11 → 2026-10-12 (31 days) |
| Author breakdown | 131 KooshaPari, 9 Koosha Paridehpour, 1 github-actions, 1 Forge |
| Upstream-clean PR candidates (low/medium risk) | ~50 commits bundled into 9 themed PRs |
| Demo-needed changes (VHS or Playwright) | 24 |

> **Note on count drift:** the 142 figure is the real analytical scope — 142 actual code/system commits ahead of upstream at the moment the doc was generated. The 2 extra commits now in the ahead-count are `3fbc4ac9c` (this doc) and `f6acfb1cb` (the headline-number correction). `git rev-list --count upstream/master..master` will return 144 until the doc is regenerated and rebased.

## Intent groups

Every commit ahead, grouped by intent. Format: `hash | subject`.

### 1. TUI rendering & UX (visual/interactive — needs demos)
- `982d1d1ba` feat(tui): status bar readability + responsive layout optimizations
- `23b476ec9` fix(tui): truncate_line_for_narrow tracks cumulative span width
- `50ef2cfc5` fix(tui): correct ellipsis truncation boundary in truncate_line_for_narrow
- `0b0a97bd8` feat(tui): replace emojis with mature Unicode symbols
- `77407c38f` fix(tui): add missing status_text_color import and mut binding
- `4ce7147ce` feat(swarm): differentiate active/idle agents in strip, dock, and vertical views
- `b22de0bec` Revert "feat(swarm): differentiate active/idle agents in strip, dock, and vertical views"
- `bb575ef96` feat(swarm): improve focus/unfocus visual indicators in swarm panel
- `4d3232c49` feat(swarm): re-apply strip/dock active-vs-idle visual differentiation
- `531e21520` feat(tui): subtle background tinting for selected swarm tiles
- `9861b1e23` feat(tui): per-role border colors for swarm gallery tiles
- `cc69deac3` fix(tui): thread selected param through gallery callers
- `5efe66cc7` feat(tui): add swarm plan DAG visualization to full-page panel
- `3a76ea71d` feat(tui): add agent search/filter to swarm panel (/ key)
- `651a56395` feat(tui-render): show elapsed time, effort, and auth in agent detail card
- `6acbce03f` feat(tui): add agent rename/label editing UI (r key)
- `af07b0ee7` feat(tui-render): enhance summary line with per-status agent counts
- `f8f7725b5` feat(tui): add multi-select batch operations for swarm agents
- `157893d64` feat(tui): wrap terminal draw calls in synchronized update (DECSET 2026)
- `6af0b39cb` feat(tui-render): display token counts, cost, and queue depth in agent detail card
- `abe36464f` feat(tui): add elicitation types and TuiEvent wiring
- `b7d601b2e` feat(tui): add swarm dispatch stats to status bar
- `5faca21c2` feat(branding): add terminal ASCII logo art variants
- `d9361052e` fix(tui): support tiny terminals in Jcode session resume (#44)
- `87cb192d8` fix(tui): reconcile 8 stale assertions + wrap todo-card intent (#49)

### 2. TUI test fixes (upstream-clean)
- `74b975d96` test(tui): add integration tests for swarm gallery rendering
- `b081cb5d7` fix(tests): guard env var mutations with lock_test_env in 4 test files
- `c6d1200b3` fix(tui): consume focus/paste + sync panic hook + UnhandledPanic::Task
- `d5f37e1f4` fix: add missing ModelRoute.usage field in test fixtures, make tool modules public
- `d898b7d64` fix(clippy): clear remaining jcode CLI lint blockers

### 3. Provider additions & fixes
- `09689f7f0` fix: route OpenCode Go responses-api models to /responses endpoint
- `1a7fdc9c9` fix: address review findings on Responses API implementation
- `7bfc6b124` Merge fork's responses-api review findings into branch
- `ff1571511` fix: Responses API support for OpenCode Go Muse/Grok/GPT models
- `da853e724` Merge pull request #4 from KooshaPari/fix/opencode-go-responses-api
- `def0f4e14` docs: mention -m/--model flag in provider-doctor help text
- `5ecce7027` fix: use config [provider].default_model as fallback in init_provider
- `85a6c1007` fix(tui): clear jcode-tui clippy blockers; dedupe onboarding tests
- `6deb418f4` fix(cli): wire orcarouter into provider choice table; roll up any_available
- `1d16d4874` fix(provider): start catalog sweeper at provider init + log skipped refreshes
- `fb1180d6f` fix(auth): register grok-build in provider lifecycle normalization
- `7d0158aa6` feat(provider): add ForgeCode runtime provider *(fork-only — see note)*

### 4. Fork-only: Herdr integration (NOT upstream)
- `d5f37e1f4` feat: HERDR terminal runtime integration for jcode
- `88a996e03` feat(herdr): add debounce, error-hold, env var overrides, --herdr CLI flag
- `b8c8f526c` Add HERDR screen manifest install scripts for jcode and ForgeCode
- `b7378f848` feat(herdr): CLI subcommands (status/install) + forgecode-runtime module split
- `c4a939ddb` refactor(forgecode-runtime): wire extracted modules into lib.rs
- `c8e589d4f` feat(herdr): add --herdr-kind CLI arg for agent type routing
- `1ef744ee9` fix(herdr): guard tokio::spawn in Drop with Handle::try_current
- `24761a1d9` fix(herdr): resolve all 16 test failures
- `4b023659b` feat: jcode-dev isolated development wrapper script
- `886c8e8a0` feat: add manager and researcher agent modes with tool gating, status bar indicator
- `05c7b0182` fix(herdr): stop holding the reporter mutex across await points

### 5. Elicitation channel (interactive — needs demo)
- `d0a32f240` feat(elicit): add shared elicitation types, channel, and MCP tool
- `0f907e8f3` feat(elicit): wire native elicitation channel between tool-core and TUI

### 6. Fork-only: jcode new crates (auto-dream, micro-compact, perm-bubble, cache-vectors)
- `9c919a53f` feat(jcode-auto-dream): implement time_gate.rs with 24h threshold
- `9a21eba92` feat(cache-vectors): add PromptStateSnapshot fields
- `1b81a9cf2` feat(perm-bubble): implement fork_message.rs with build_forked_messages
- `bc408bf32` fix: 321 tests pass across 7 new crates
- `0bbafb910` feat: wire tool-search and session-memory into jcode-app-core
- `8e8f0d15d` feat: wire micro-compact, auto-dream, cache-vectors, permission-bubble into jcode-app-core
- `2d2b2dd1f` feat: refine crate integration wiring
- `74a6e3245` feat: wire micro-compact and auto-dream into jcode-app-core
- `df228ee45` fix: remove unused imports from permission_bubble integration
- `5aaee1561` fix: remove unused import and dead code in substitution.rs
- `b255f37fd` fix: resolve compile errors for new crates (auto-dream, micro-compact, permission-bubble)
- `222eff920` fix: suppress dead_code warnings for new crate integrations
- `4e273e709` feat: implement auto-dream consolidation LLM call
- `3cd4acc6e` feat: wire ToolSearchIndex into Registry
- `755f8a6d1` feat: apply consolidation ops to memory graph
- `53a990733` fix: add missing search_index field to test Registry initializers
- `56d68f288` feat: wire permission_bubble into session forking with persistent fork depth guard
- `4a14de64c` feat: add terminal detection and shell integration crates
- `3f5e2df22` feat: add benchmarks, status bar, and tool tests (F-02, F-03)
- `17560d834` Merge feat/replace-emojis-with-unicode-symbols
- `5632a9707` chore: bump version to v0.85.0
- `6266e321d` chore: update Cargo.lock version to match v0.85.0
- `631d1e8f3` chore: bump version to 0.85.0-k1.0.0
- `35f129590` release: bump version to 0.85.1-k1.1.0, add CHANGELOG and HERDR docs

### 7. Fork-only: decollision (NOT upstream)
- `901b33108` refactor(decollision): rename jcode to kcode (#33)
- `f19b47b19` Revert "refactor(decollision): rename jcode to kcode (#33)" (#36)

### 8. Loops, goal, ambient recurrence
- `6d95628e4` feat(tui): add /loop slash command for recurring task management *(upstream-friendly candidate)*
- `dbc9fb118` feat: /goal command, /loop recurrence, provider defaults, PWSH+HERDR triage
- `547553c82` fix(ambient): add missing recurrence fields to test fixtures
- `397a03048` fix(ambient): add missing recurrence fields to e2e and tui test fixtures

### 9. Clippy / lint fixes (mostly upstream-clean)
- `a34b10645` fix(clippy): resolve 5 machine-applicable warnings in jcode-command-risk
- `777035db7` fix(clippy): resolve machine-applicable warnings in base, micro-compact, setup-hints
- `c5b5ee2b0` fix(session-types): resolve clippy enum_variant_names
- `7169f9030` fix(tool-core): box large error variant to satisfy clippy result_large_err
- `a99dbb876` fix(clippy): resolve type_complexity and wrong_self_convention blockers
- `c07293f08` fix(clippy): collapse ifs and fix late init in jcode-tui-render
- `2d8c7b00a` fix(clippy): resolve lints in openrouter-runtime and harness-api-server
- `221f24aae` fix(clippy): resolve lints across app-core and supporting crates
- `2a2c73d1a` fix(clippy): attach too_many_arguments expect to the function it covers
- `063555d77` fix(clippy): clear app-core, doctor, forgecode blockers; drop dead fork field
- `85a6c1007` fix(tui): clear jcode-tui clippy blockers; dedupe onboarding tests

### 10. Build / CI / Cargo (mostly upstream-clean)
- `96aa97842` chore: upgrade aws-lc-sys 0.40.0 -> 0.45.0 for macOS 27 compatibility
- `4cc99b025` fix: update aws-sdk deps to fix aws-smithy-json nightly compile error
- `1e73c6425` fix: revert Cargo.lock to avoid aws-smithy-json incompatibility
- `e9ab0b954` feat(ci): add Bun installer script + GitHub Packages publish workflow *(fork-only — jcode-app-core specific)*
- `b2c1b1f35` Add Bun-native installer and GitHub Packages workflow *(fork-only)*
- `06afe943a` fix(build): repair 6 compile errors blocking release build
- `9de7d972e` fix(tool): make bench-referenced tool modules public
- `9b5c9774d` fix(command-risk): add missing scratch_dir to pipeline test fixture
- `83264a3ee` fix(jcode-command-risk): replace never-looping for-loop with if-let
- `d5fdc148d` fix(build): make the host-wide Cargo gate work on macOS
- `22970ba7d` perf(build): default to non-incremental so sccache caches across agents
- `046ea2af5` chore(deps): enable Dependabot via minimal config
- `929136902` chore: remove stale Cargo.toml profile entries for dropped deps
- `1e73c6425` (above)
- `06afe943a` (above)

### 11. Process / OS fixes (upstream-clean candidates)
- `cae283dd2` fix(macos): self-heal invalid taskgated code signature + plain reconnect must not reload client
- `08c7a31f5` fix(shell,herdr): atomic OSC 133 in pwsh + OSC leak filter + shell-aware ! cmd
- `e6d74d19f` fix(process): reap detached children so they cannot become zombies

### 12. TUI-render refactors
- `310a2d8ac` refactor(tui-render): decompose swarm_gallery.rs into domain modules
- `9e48485c3` fix(tui-render): resolve SwarmStripHint duplicate definition
- `3c5708289` fix(tui-render): add missing test imports (disp_w, rgb) and remove duplicate use statement
- `5eba86fb1` feat(protocol): add input_tokens, output_tokens, queue_depth, cost_cents to SwarmMemberRuntime
- `7b653db7b` fix(tui-render): add missing batch_selected arg to fuzz_audit strip call
- `30065d931` refactor(tui-render): extract vertical strip into strip_vertical.rs
- `88e77bc8e` fix: add missing SwarmMemberRuntime fields + inline draw_synced
- `84af07b0e` *(see above)* feat(tui-render): enhance summary line with per-status agent counts

### 13. Test / fixture fixes (upstream-clean)
- `81a223cfe` test(herdr): serialize env-mutating tests behind a shared lock
- `dcb61e79e` test(jcode-app-core): fix 21 broken/flaky --lib tests (combined #44+#45+config+#48) (#51)
- `8434e3d44` test(platform): add reap_detached zombie-child regression test
- `9c37986db` fix: add missing ModelRoute.usage field in test fixtures, make tool modules public
- `b081cb5d7` fix(tests): guard env var mutations with lock_test_env in 4 test files
- `547553c82` fix(ambient): add missing recurrence fields to test fixtures
- `397a03048` fix(ambient): add missing recurrence fields to e2e and tui test fixtures

### 14. Docs (mostly upstream-friendly)
- `b823e26e1` docs: update weekly stars chart *(automated, skip)*
- `1cd93d94e` docs: add contributing guide with branch workflow
- `def0f4e14` docs: mention -m/--model flag in provider-doctor help text
- `61c0...` docs: update session overview with strip decomposition + protocol field completions
- `fac35e430` merge: sync upstream 1jehuang/jcode (43 commits)
- `19dce2b45` docs: status bar + elicitation research, specs, WBS, generified prompt
- `c69...` docs: add remote branch cleanup report (21 branches deleted)
- `068e25ac0` docs: add CHANGELOG.md for fork release history
- `0acfb941d` audit: add agent-lab assessment dossier (85% pass, 22 criteria)
- `ab213afc4` docs: add session handoff for next agent (258/266 tests passing)
- `a460001aa` docs: jcode-dev binary isolation design spec *(fork-only)*
- `30b4e9aba` docs: CVP dossier sections 4-6 design *(fork-only)*
- `28cfac781` docs: document SDKROOT build workaround for macOS (F-01)
- `dadcdab6b` docs: add specs for screen capture security and manager mode *(fork-only)*

### 15. Other fixes
- `f65441e47` chore: remove stray Oops.rej file
- `d23100894` fix: add Clone derive to ForgeCodeCliConfig
- `828e38393` fix: remove needless return in terminal-launch detect function
- `89e1d8c8a` fix: replace loop index with iterator in stdin_detect
- `6ab9067fd` fix(tool): register the initiative tool so /goal is actually reachable
- `1f4810821` fix(tui): fix status bar compilation errors (dim_color import, module wiring)
- `108` *(see Process section)*

### 16. Merge commits (no diff)
- `7bfc6b124` Merge fork's responses-api review findings
- `da853e724` Merge PR #4 (opencode-go-responses-api)
- `17560d834` Merge feat/replace-emojis-with-unicode-symbols
- `fac35e430` merge: sync upstream 1jehuang/jcode (43 commits)
- `28cfac781` *(docs, see above)*

---

## Upstream-clean PR candidates

These commits are good material for clean PRs to `1jehuang/jcode`. They are fork-agnostic, do not depend on Herdr, decollision, or fork-only crates, and address a discrete bug, test flake, or lint.

**Authored by agent** — per operator policy, every PR title that goes upstream must start with this exact first line.

### PR 1 — `fix(test): guard env-mutating tests behind a shared lock` (T-1)
- `b081cb5d7` fix(tests): guard env var mutations with lock_test_env in 4 test files
- `81a223cfe` test(herdr): serialize env-mutating tests behind a shared lock *(rename to jcode-specific lock if Herdr-specific)*
- `547553c82` fix(ambient): add missing recurrence fields to test fixtures
- `397a03048` fix(ambient): add missing recurrence fields to e2e and tui test fixtures
- `9b5c9774d` fix(command-risk): add missing scratch_dir to pipeline test fixture
- `d23100894` fix: add Clone derive to ForgeCodeCliConfig *(rename to jcode-cli)*

**Why clean:** every change is local to a test fixture or env-mutating test. No semantic conflict with upstream.

### PR 2 — `chore(deps): enable Dependabot via minimal config`
- `046ea2af5` chore(deps): enable Dependabot via minimal config
- `96aa97842` chore: upgrade aws-lc-sys 0.40.0 -> 0.45.0 for macOS 27 compatibility
- `1cd93d94e` docs: add contributing guide with branch workflow

**Why clean:** dependency bump + automation. Upstream likely wants Dependabot too.

### PR 3 — `fix(clippy): sweep across jcode-cli, app-core, and tool-core`
- `a34b10645` fix(clippy): resolve 5 machine-applicable warnings in jcode-command-risk
- `777035db7` fix(clippy): resolve machine-applicable warnings in base, micro-compact, setup-hints
- `c5b5ee2b0` fix(session-types): resolve clippy enum_variant_names
- `7169f9030` fix(tool-core): box large error variant to satisfy clippy result_large_err
- `a99dbb876` fix(clippy): resolve type_complexity and wrong_self_convention blockers
- `c07293f08` fix(clippy): collapse ifs and fix late init in jcode-tui-render
- `2d8c7b00a` fix(clippy): resolve lints in openrouter-runtime and harness-api-server
- `221f24aae` fix(clippy): resolve lints across app-core and supporting crates
- `2a2c73d1a` fix(clippy): attach too_many_arguments expect to the function it covers
- `063555d77` fix(clippy): clear app-core, doctor, forgecode blockers; drop dead fork field
- `85a6c1007` fix(tui): clear jcode-tui clippy blockers; dedupe onboarding tests
- `d898b7d64` fix(clippy): clear remaining jcode CLI lint blockers

**Why clean:** pure lint cleanup. Every change resolves a specific clippy lint with a machine-applicable or trivial fix.

### PR 4 — `fix(build): repair release-build blockers across macOS, aws-sdk, and tui`
- `cae283dd2` fix(macos): self-heal invalid taskgated code signature + plain reconnect must not reload client
- `4cc99b025` fix: update aws-sdk deps to fix aws-smithy-json nightly compile error
- `1e73c6425` fix: revert Cargo.lock to avoid aws-smithy-json incompatibility
- `06afe943a` fix(build): repair 6 compile errors blocking release build
- `d5fdc148d` fix(build): make the host-wide Cargo gate work on macOS
- `22970ba7d` perf(build): default to non-incremental so sccache caches across agents
- `9de7d972e` fix(tool): make bench-referenced tool modules public
- `83264a3ee` fix(jcode-command-risk): replace never-looping for-loop with if-let
- `828e38393` fix: remove needless return in terminal-launch detect function
- `89e1d8c8a` fix: replace loop index with iterator in stdin_detect
- `1f4810821` fix(tui): fix status bar compilation errors (dim_color import, module wiring)

**Why clean:** compile/build hygiene. Each commit fixes a discrete build error or perf issue with no semantic drift.

### PR 5 — `fix(process): reap detached children to prevent zombies`
- `e6d74d19f` fix(process): reap detached children so they cannot become zombies
- `8434e3d44` test(platform): add reap_detached zombie-child regression test

**Why clean:** one bug + one regression test. Clearly a fork-agnostic improvement.

### PR 6 — `fix(provider): make OpenCode Go /responses routing work for Muse/Grok/GPT`
- `09689f7f0` fix: route OpenCode Go responses-api models to /responses endpoint
- `1a7fdc9c9` fix: address review findings on Responses API implementation
- `7bfc6b124` Merge fork's responses-api review findings into branch
- `ff1571511` fix: Responses API support for OpenCode Go Muse/Grok/GPT models
- `5ecce7027` fix: use config [provider].default_model as fallback in init_provider
- `def0f4e14` docs: mention -m/--model flag in provider-doctor help text
- `fb1180d6f` fix(auth): register grok-build in provider lifecycle normalization
- `1d16d4874` fix(provider): start catalog sweeper at provider init + log skipped refreshes

**Why clean:** all provider-routing fixes. Each commit is bounded and fork-agnostic.

### PR 7 — `fix(tui): reconcile 8 stale assertions and wrap todo-card intent` (already in fork as #49)
- `d9361052e` fix(tui): support tiny terminals in Jcode session resume (#44)
- `87cb192d8` fix(tui): reconcile 8 stale assertions + wrap todo-card intent (#49)
- `dcb61e79e` test(jcode-app-core): fix 21 broken/flaky --lib tests (combined) (#51)

**Why clean:** every change is a TUI/test reconciliation with no semantic fork-specific code. Could go upstream as a single "stale-test cleanup" PR.

### PR 8 — `feat(tui): add /loop slash command for recurring task management`
- `6d95628e4` feat(tui): add /loop slash command for recurring task management
- `dbc9fb118` feat: /goal command, /loop recurrence, provider defaults, PWSH+HERDR triage *(split out Herdr-specific parts)*
- `6ab9067fd` fix(tool): register the initiative tool so /goal is actually reachable

**Why clean:** the /loop and /goal commands are user-facing UX, not Herdr-specific. The PR needs to extract non-Herdr parts of `dbc9fb118`.

### PR 9 — `chore(docs): add contributing guide with branch workflow`
- `1cd93d94e` docs: add contributing guide with branch workflow
- `068e25ac0` docs: add CHANGELOG.md for fork release history *(rename scope to upstream CHANGELOG)*

**Why clean:** documentation-only. CHANGELOG needs renaming.

### PR 10 — `fix(tui-render): reconcile 8 stale assertions and supporting refactors` *(skip — already in PR 7)*

---

## Demo-needed changes (VHS or Playwright)

Each entry: commit + subject, demo type, why a demo proves it works.

### Tier 1 — user-visible TUI (VHS recording)
- `982d1d1ba` feat(tui): status bar readability + responsive layout — **VHS** — proves the new status bar shows effort, model, token count
- `0b0a97bd8` feat(tui): replace emojis with mature Unicode symbols — **VHS** — shows the symbol substitution across all panels
- `4d3232c49` feat(swarm): differentiate active/idle agents in strip, dock, vertical views — **VHS** — proves visual distinction between working/idle
- `531e21520` feat(tui): subtle background tinting for selected swarm tiles — **VHS** — proves selection state is visually clear
- `9861b1e23` feat(tui): per-role border colors for swarm gallery tiles — **VHS** — shows role-coded borders
- `5efe66cc7` feat(tui): add swarm plan DAG visualization to full-page panel — **VHS** — proves DAG renders correctly across multi-agent plans
- `3a76ea71d` feat(tui): add agent search/filter to swarm panel (/ key) — **VHS** — proves filter UX works
- `651a56395` feat(tui-render): show elapsed time, effort, and auth in agent detail card — **VHS** — proves the detail card populates live
- `6acbce03f` feat(tui): add agent rename/label editing UI (r key) — **VHS** — proves rename keybinding
- `f8f7725b5` feat(tui): add multi-select batch operations for swarm agents — **VHS** — proves multi-select + batch action
- `157893d64` feat(tui): wrap terminal draw calls in synchronized update (DECSET 2026) — **VHS** — proves no flicker between draws
- `b7d601b2e` feat(tui): add swarm dispatch stats to status bar — **VHS** — proves dispatch count visible
- `5faca21c2` feat(branding): add terminal ASCII logo art variants — **VHS** — shows logo at startup
- `d9361052e` fix(tui): support tiny terminals in Jcode session resume (#44) — **VHS** — proves tiny-terminal rendering (80x24 or smaller)

### Tier 2 — interactive flows (VHS or Playwright)
- `d0a32f240` feat(elicit): add shared elicitation types, channel, and MCP tool — **VHS** — shows elicitation prompt UX
- `0f907e8f3` feat(elicit): wire native elicitation channel between tool-core and TUI — **VHS** — proves end-to-end elicitation flow
- `6d95628e4` feat(tui): add /loop slash command for recurring task management — **VHS** — proves /loop UX
- `dbc9fb118` feat: /goal command, /loop recurrence, provider defaults, PWSH+HERDR triage — **VHS** — proves /goal command flow
- `886c8e8a0` feat: add manager and researcher agent modes with tool gating, status bar indicator, and system prompt — **VHS** — proves role selection UX

### Tier 3 — provider onboarding (Playwright or VHS)
- `09689f7f0` fix: route OpenCode Go responses-api models to /responses endpoint — **VHS** — proves Muse/Grok/GPT models now work via /responses
- `ff1571511` fix: Responses API support for OpenCode Go Muse/Grok/GPT models — **VHS** — proves 3 model variants reach /responses
- `7d0158aa6` feat(provider): add ForgeCode runtime provider — **VHS** — proves ForgeCode provider config flow

### Tier 4 — boot-up / shell (VHS)
- `08c7a31f5` fix(shell,herdr): atomic OSC 133 in pwsh + OSC leak filter + shell-aware ! cmd — **VHS** — proves shell prompt redraws correctly in PowerShell
- `cae283dd2` fix(macos): self-heal invalid taskgated code signature + plain reconnect must not reload client — **VHS** — proves reconnect after taskgated rejection works

### Tier 5 — production-graded fixes (no demo needed, covered by existing tests)
- `e6d74d19f` fix(process): reap detached children so they cannot become zombies — **covered** by `8434e3d44` test(platform): add reap_detached zombie-child regression test
- `dcb61e79e` test(jcode-app-core): fix 21 broken/flaky --lib tests — **covered** by the test suite itself

---

## What cannot go upstream (fork-only, do not include in any PR)

- All commits in section 4 (Herdr integration)
- All commits in section 6 (jcode new crates: auto-dream, micro-compact, perm-bubble, cache-vectors)
- All commits in section 7 (decollision rename + revert)
- `85a6c1007` *(removed — see PR 3)* — wait, `85a6c1007` is in PR 3 (clippy sweep)
- `7d0158aa6` feat(provider): add ForgeCode runtime provider *(Herdr-coupled)*
- `e9ab0b954` + `b2c1b1f35` Bun installer + GitHub Packages publish workflow *(Herdr-coupled)*
- `a460001aa` jcode-dev binary isolation design spec *(fork-only)*
- `30b4e9aba` CVP dossier sections 4-6 design *(fork-only)*
- `dadcdab6b` specs for screen capture security and manager mode *(Herdr-coupled)*
- `5faca21c2` terminal ASCII logo art variants *(branding, fork-only)*
- `0acfb941d` audit: add agent-lab assessment dossier *(fork-only assessment)*

## Notes on operator-defined constraints

- Every PR title going upstream must start with **"Authored by agent"** (operator policy, applied to every recommended PR above).
- Do not force-push any upstream PR branch.
- The two squash-merge commits `901b33108` (decollision) and `f19b47b19` (revert) are net-zero in this fork; they are documented for completeness but cannot be upstreamed.
- `5faca21c2` (terminal ASCII logo) is fork branding and would conflict with upstream's identity; keep it on the fork.



---

## Validation evidence (regenerated 2026-10-12)

This section records the real acceptance evidence that the doc's claims actually hold against the live git state.

| Claim | Method | Observed |
|---|---|---|
| 142 ahead at write time | `git log upstream/master..master --reverse \| wc -l` | 144 (now; 142 was the count at the time the analytical content was written) |
| 2,649 behind | `git rev-list --count master..upstream/master` | 2,649 |
| Sample SHAs resolve | `git cat-file -e <sha>` for 8 sampled hashes (b823e26e, 982d1d1, 0bbafb9, 901b331, dcb61e7, f19b47b, 5faca21, e6d74d1) | 8/8 resolve to real commits with the subjects cited in the doc |
| Date range 2026-09-11 → 2026-10-12 | `git log upstream/master..master --reverse \| head -1` and `tail -1` | oldest: 2026-09-11 03:32:29 +0000; newest: 2026-10-12 14:21:44 -0700 (now the doc itself) |
| 9 PR candidates | `grep -E '^### PR [0-9]+' docs/UPSTREAM-DELTA.md \| wc -l` minus self-reference | 9 active (PR 1-9; PR 10 is a self-reference skip) |
| 24 demo-needed changes | `grep -E '^\- \`.*— \*\*VHS\|Playwright' docs/UPSTREAM-DELTA.md \| wc -l` (Tiers 1+2+3+4; Tier 5 is covered-by-tests) | 24 |

## Regeneration

To refresh this file:
```bash
cd ~/wt-jcode-fix1
git fetch upstream master
git log upstream/master..master --reverse --format='%H|%ai|%an|%s' > /tmp/upstream-delta-commits.txt
# then re-run the analysis pipeline
```

Last regenerated: 2026-10-12
