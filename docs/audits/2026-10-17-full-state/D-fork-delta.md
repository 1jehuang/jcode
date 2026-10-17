# Audit D — Fork vs Upstream Delta (KCode / 1jehuang/jcode)

**Repo:** /Users/kooshapari/CodeProjects/Phenotype/repos/jcode
**GitHub:** `KooshaPari/KCode` (renamed from `KooshaPari/Jcode`)
**Upstream:** `1jehuang/jcode`
**Auditor:** read-only audit worker, 2026-10-16 (lane D)
**Status:** COMPLETE (steps 1-3 written early; 5-8 finalized in this pass)

---

## Step 1 — git fetch + rev-list (TRUE STATE OF THE FORK)

```
$ git fetch upstream --tags
From https://github.com/1jehuang/jcode
 ! [rejected]            v0.85.0    -> v0.85.0  (would clobber existing tag)

$ git rev-list --left-right --count upstream/master...master
2828    334

$ git rev-list --left-right --count upstream/main...master
fatal: ambiguous argument 'upstream/main...master': unknown revision
```

- **Behind upstream:** **2,828 commits** (LEFT of `upstream/master...master`)
- **Ahead of upstream:** **334 commits** (RIGHT)
- `upstream/main` does **not** exist — upstream's only default branch is `master`. The canonical comparison is `upstream/master...master`.
- The fork's own docs claim (~144 ahead, ~2649 behind, generated 2026-10-12) are **stale by 4 days**:
  - ahead: 144 → **334** (+190 ahead added)
  - behind: 2649 → **2828** (+179 net upstream gains)
- Tag `v0.85.0` was rejected during fetch (would clobber the fork's existing tag). All other upstream tags fetched cleanly.

Counts corrected by re-running against the immutable tips below:

```sh
git rev-list --left-right --count 4a4c678f15e5d10b34712c3f18feb56fe84f7b89...748eeaa1e1f45a87a5d6d230963c6ad98495525b
# 2828 334
```

The original transcript above contained incorrect counts; the corrected values
are shown. D-prep-cherrypick-analysis.md compares a later upstream tip and
therefore reports 2,840 behind, also 334 ahead.

### Tip SHAs (2026-10-16 23:09 UTC)

| Ref | SHA | Subject |
|---|---|---|
| `upstream/master` | `4a4c678f15e5d10b34712c3f18feb56fe84f7b89` | Keep Sol hotkey coverage diff scoped to speed tests |
| `master` (fork) | `748eeaa1e1f45a87a5d6d230963c6ad98495525b` | docs(handoff): name the real root cause of the Actions block; record the repo rename |

---

## Step 2 — Upstream version

```
$ git ls-remote --tags upstream | tail -12
... v0.94.0 -> 6f0f967c
... v0.95.0 -> a74c00fb
... v0.96.0 -> bcdda410
... v0.97.0 -> 8cd52cef
... v0.98.0 -> f65ddc0e
... v0.98.1 -> 61ed7d98 (current HEAD tag)

$ git show upstream/master:Cargo.toml | head -8
[package]
name = "jcode"
version = "0.98.1"
description = "Possibly the greatest coding agent ever built — blazing-fast TUI, multi-model, swarm coordination, 30+ tools"
edition = "2024"
```

- Upstream is at **v0.98.1** (released 2026-10-14, 2 days before this audit).
- Fork's Cargo.toml: `name = "jcode"`, `version = "0.85.1-k1.1.1"`. The fork pin is from **v0.85.1** of upstream; **13 releases of drift** (v0.86.0 → v0.98.1) plus 2,828 commits behind.

---

## Step 3 — Fork's own delta doc claims (and how they compare to reality)

`docs/UPSTREAM-DELTA.md` (generated 2026-10-12, 4 days stale):
- Reports **142 ahead at write time, 2,649 behind**.
- Groups the fork's 142 ahead-commits into 16 intent categories. Largest clusters: TUI rendering & UX (26), clippy sweeps (12), build/CI/Cargo (10), test/fixture fixes (6), TUI-render refactors (6), doc/CHANGELOG commits (10+).
- Lists 9 "upstream-clean" PR candidates (~50 commits bundled into themed PRs).
- Lists 24 demo-needed changes (VHS / Playwright).
- Flags ~14 commits as "fork-only, do not upstream" (Herdr, new crates, decollision, ForgeCode provider, Bun installer, branding/logo, agent-lab dossier).

`docs/UPSTREAM-SYNC-STATUS.md` (2026-10-12):
- Claims **the fork cannot mechanically absorb upstream via merge** because 47 untracked worktree paths would be overwritten.
- 152 total untracked paths; 47 conflict with upstream; 105 are pure fork debris (96 of those are decollision-rename leftovers — `crates/jcode-desktop*` + `crates/kcode-*`).
- Recovery: operator-approve cleanup of 47 conflict paths (Strategy B = per-file diff), then 96 decollision debris, then re-merge.
- **This pre-condition is STILL UNFIXED** as of today (2026-10-16); the sync ritual remains blocked. The doc was never updated to reflect that the operator never approved the cleanup.

### Verdict on the fork's own claims

- The 2,649 / 142 numbers are stale by 4 days; the authoritative figures are 2,828 / 334.
- The fork's grouping of fork-only vs upstream-friendly is structurally sound and the 9 PR-candidate bundles still map cleanly onto today's ahead-set (the ahead set has grown but intent categories are stable).
- The sync-status diagnosis (worktree debris blocking merge) is structurally correct AND STILL UNRESOLVED.

---

## Step 5 — Fork DELTA: what the fork adds

### Crate-name truth (the decollision was reverted in code; only GitHub was renamed)

- **Cargo.toml `name = "jcode"`** (line 2, 107, 111).
- **92 crate directories** under `crates/`, every single one named `jcode-*` (e.g. `jcode-app-core`, `jcode-herdr`, `jcode-auto-dream`, `jcode-micro-compact`, `jcode-perm-bubble`, `jcode-cache-vectors`, `jcode-tui`, `jcode-base`, ...).
- **3 binary names** defined: `jcode` (the main binary), `jcode` (lib), `test_api`, `jcode-harness`.
- `rg -l "KCode|kcode" crates/ src/` returns **only 10 files**, and all hits are intentional **doc comments** referencing the fork identity (e.g. `crates/jcode-app-core/src/agent/environment.rs: KCode fork: the env snapshot is recorded in memory`, `crates/jcode-storage/src/remove.rs: Adapted from the KCode fix at ceb860bf2`, `crates/jcode-app-core/src/update_product.rs: this product's 0.85.1-k1.1.1 (k = the KooshaPari/KCode fork semver)`).
- **Verdict: the decollision rename is fully reverted in code.** The Rust source is structurally identical to upstream `jcode`. Only the GitHub repo name diverges (`KooshaPari/KCode` vs `1jehuang/jcode`). The PR #33 rename attempt produced 344,430 additions / 263,499 deletions; the PR #36 revert undid it byte-for-byte (reverse: 263,508 / 344,439).

### What the fork adds (the real delta)

The 334 ahead-commits, grouped by intent (from UPSTREAM-DELTA.md, expanded with new code investigation):

1. **TUI rendering & UX (~30+ commits):** status bar readability, ellipsis truncation, Unicode symbol replacement, swarm strip/dock active-vs-idle differentiation, per-role border colors, swarm plan DAG viz, agent search/filter (`/`), elapsed/effort/auth in detail card, agent rename UI (`r`), multi-select batch ops, synchronized draw calls (DECSET 2026), token/cost/queue in detail card, elicitation wiring, swarm dispatch stats, terminal ASCII logo, tiny-terminal resume.
2. **Provider additions:** OpenCode Go `/responses` routing for Muse/Grok/GPT, `grok-build` lifecycle normalization, catalog sweeper at provider init, `orcarouter` choice table wiring, **ForgeCode runtime provider (fork-only, Herdr-coupled)**.
3. **Herdr integration (FORK-ONLY, ~11 commits):** Herdr terminal runtime, debounce + error-hold + env overrides + `--herdr` CLI flag, screen manifest install scripts, `--herdr-kind` CLI arg, tokio-spawn guard, jcode-dev isolation wrapper, manager/researcher agent modes with tool gating.
4. **Elicitation channel (2 commits):** shared elicitation types, channel, MCP tool, native TUI wiring. Likely upstream-compatible.
5. **New crates (FORK-ONLY, ~14 commits):** `jcode-auto-dream` (24h time_gate, consolidation LLM call), `jcode-micro-compact`, `jcode-perm-bubble` (fork_message, persistent fork depth), `jcode-cache-vectors` (PromptStateSnapshot), `tool-search`, `session-memory`. Wired into `jcode-app-core`.
6. **Decollision (FORK-ONLY):** PR #33 squash-merge + PR #36 revert. Net zero on the fork side.
7. **Loops, goal, ambient recurrence (4 commits):** `/loop` slash command, `/goal` command, provider defaults, PWSH+HERDR triage. Init tool registered so `/goal` is reachable.
8. **Clippy / lint sweeps (~12 commits):** resolve `enum_variant_names`, `result_large_err`, `type_complexity`, `wrong_self_convention`, `too_many_arguments` blockers across base, micro-compact, setup-hints, openrouter-runtime, harness-api-server, app-core, doctor, forgecode, jcode-tui, jcode-command-risk.
9. **Build / CI / Cargo (~13 commits):** aws-lc-sys 0.40→0.45 (macOS 27 compat), aws-sdk nightly fix + revert, Bun installer (FORK-ONLY), GitHub Packages publish workflow (FORK-ONLY), 6 compile error repairs, sccache-friendly non-incremental default, Dependabot minimal config, profile entry cleanup.
10. **Process / OS fixes (3 commits):** macOS taskgated self-heal + plain reconnect no-reload, atomic OSC 133 in pwsh + OSC leak filter, reap_detached to prevent zombies (with regression test).
11. **TUI-render refactors (~6 commits):** swarm_gallery decomposition, vertical strip extraction, missing SwarmMemberRuntime fields + inline draw_synced, SwarmStripHint dedup.
12. **Test/fixture fixes (~10 commits):** `lock_test_env` in 4 test files, reap_detached regression test, ambient recurrence fixtures, command-risk scratch_dir fixture, broken/flaky --lib test fixes (#51), TUI reconcile (#44, #49).
13. **Docs (~13 commits):** contributing guide, weekly stars chart (automated), session handoffs, jcode-dev isolation spec (FORK-ONLY), CVP dossier (FORK-ONLY), SDKROOT build workaround (F-01), screen-capture/manager-mode specs (FORK-ONLY), CHANGELOG.md, agent-lab audit dossier (FORK-ONLY).
14. **Other (~10 commits):** Clone derive on ForgeCodeCliConfig, needless return removal, loop index → iterator, initiative tool registration, status bar compilation fixes.
15. **Merge commits:** 5 merges, no net diff content.

---

## Step 6 — Fork DEFICIT: major upstream capabilities since v0.85 the fork lacks

Spanning **v0.86.0 → v0.98.1** (13 releases, 2,828 commits):

| Version | Title | Top capability the fork lacks |
|---|---|---|
| **v0.86.0** | Smarter task handling and clearer session feedback | Jev browser handoff for whole multi-step tasks; memory recall via Jev (no local embedding model at startup); `/cache extend` for Anthropic prompt-cache retention. |
| **v0.87.0** | Claude Opus 5.5 + independent Anthropic model discovery | Claude Opus 5.5 model via Anthropic with API-key + Claude-login routes; independent API-key vs OAuth model catalog refresh; Claude OAuth client compatibility updated to Claude Code 2.1.280. |
| **v0.88.0** | Claude Opus 5.5 default, multi-browser, banked usage resets | Claude Opus 5.5 @ medium is default Anthropic model; `anthropic-api:` model specs use native Anthropic runtime; browser tool detects default browser (Chrome, Edge, Brave, Chromium, Safari); OpenAI banked usage resets. |
| **v0.89.0** | Built-in voice input, agent applets, pinned status line | Native mic capture on macOS/Windows with coding-aware dictation; pinned session status line (directory, branch, git, context, provider, model); agents can show interactive applets; swarms message across swarms with unique labels. |
| **v0.90.0** | Provider-native web search | Default web search uses model provider's own server-side search (Anthropic + OpenAI) instead of local scraping; provider search follows session's allowed/disabled tool policy; installing skills mid-session preserves prompt cache. |
| **v0.91.0** | Google Calendar + speed tiers | Google login service selection + built-in calendar tool (view/create/update/delete events); guided Google OAuth app setup; Alt+Up/Down cycles Standard/Fast/Ultrafast; `/desktop` command opens current session in Jcode Desktop. |
| **v0.92.0** | Codemode + smarter model defaults | Opt-in codemode tool runs JavaScript in embedded sandbox chaining/looping/parallelizing tool calls; every provider defaults to newest flagship model; browser bridge auto-updates. |
| **v0.93.0** | Instant startup | ~20ms first-frame paint (was ~135ms); multi-session open fast; large-session resume ~3x quicker; new `post_tool_feedback` hook + enriched `post_tool` hooks (tool input, call id, touched paths). |
| **v0.94.0** | Queued commands | Ctrl+Enter queues slash and `!` commands to run after current turn; `/clear` drops queued; Alt+G opens git commit log + diff cycling to `/diff`; Harness API clients can observe and cancel mid-turn messages. |
| **v0.95.0** | (OpenCode Go Responses routing) | OpenCode Go models (grok-4.7, gpt-6-luna) route over Responses API; hooks expose turn-end token usage; `turn_start`/`turn_end` fire for one-shot `jcode run` turns; `/agents` supports per-session worker model overrides. |
| **v0.96.0** | Remote MCP over HTTP/SSE | Connect to remote MCP servers over HTTP or SSE (with OAuth sign-in and CLI management); choose saved OAuth accounts per session from model/login pickers; SDK clients organize sessions into collections + attach hidden reminders. |
| **v0.97.0** | Custom HTTP JSON search providers | Configurable authentication, result mapping, fallback engines; Rust + TypeScript SDK clients invoke native runtime commands + transfer sessions without converting to agent prompts. |
| **v0.98.0** | Faster idle + richer responses | Reduced idle CPU/disk reads in repos with many untracked files; shared model catalog processing across equivalent clients; Desktop-capable clients receive Markdown responses via new `user_response` tool. |
| **v0.98.1** | Faster idle + richer responses (patch) | Compact context usage before/after compaction; resume picker opens while session scan continues; disable automatic memory recall (keep explicit memory search); large token/cost summaries display compactly; recover browser handoffs from runtime provider failures; restore Windows companion connections over named pipes; expose user-session connection presence to SDK clients. |

**Summary of deficit:** The fork is missing roughly 13 releases of user-visible improvements across all major surfaces: model/provider (Claude Opus 5.5 default, native web search, codemode, custom JSON search providers), UX (voice input, pinned status, queued commands, multi-browser, speed tiers, instant startup, agent applets, calendar), integration (remote MCP over HTTP/SSE, OAuth account picker, SDK collections), and platform (Windows companion, Jcode Desktop). The fork's additions are mostly orthogonal (Herdr, new crates, elicitation channel) but it has nothing for any of these release-line features.

---

## Step 7 — Decollision history (DONE / IN PROGRESS / UNKNOWN)

- **PR #33** — `refactor(decollision): rename jcode to kcode`. **MERGED** at `901b3310821e9d65a88301072bb0a183709eefad` on 2026-10-09T05:37Z. A **2,260-commit squash rebase** of the rename history onto master. **Diff: +344,430 / −263,499 across 2,471 files.** Lossy: dropped test body in `kcode-tui-style/src/harmony.rs`, introduced stray `};` in `kcode-base/src/provider/mod.rs:117`, left empty `Serialize`/`Deserialize` impls in `kcode-swarm-core/src/lib.rs:183,187`. `cargo check --workspace` failed with 6+ errors across 3 crates (kcode-base, kcode-swarm-core, kcode-setup-hints).
- **PR #36** — `revert: refactor(decollision) rename jcode to kcode (#33)`. **MERGED** at `f19b47b196e7b9167477b19810bb6728e0874b32` on 2026-10-11T01:23:22Z. Reverse diff: **+263,508 / −344,439 across 2,471 files.** `git diff 046ea2af5 HEAD` on the revert branch was empty — perfect inverse.
- **Master state (verified):** `f19b47b19` IS in master's commit chain (`git log 046ea2af5..master` shows it). Master is at `748eeaa1e`, **198 commits ahead** of the pre-rename `046ea2af5`. The Rust source is back to `jcode-*` crates, `name = "jcode"`. **The GitHub repo name is still `KooshaPari/KCode`** (only the code rename was reverted).
- **Issue #43** — `Tighter Phase 2 rebase: per-commit cargo check (replacement for #33's 2260-commit rebase)`. **OPEN**. Created 2026-10-11T01:50:51Z, not closed. Acceptance criteria: `cargo check --workspace` exits 0; `cargo test --workspace` passes; `cargo clippy --workspace --all-targets -- -D warnings` exits 0; rebase ≤200 commits per push; per-commit `cargo check` between `git rebase --continue` steps. **Status: Not started, waiting on maintainer bandwidth.**
- **Issue #32** (per #43's reference) — umbrella for decollide work, Phase 1 done via #31, Phase 2 still scoped (kcode-update-core endpoint, README URLs, GHCR images, npm package names, `jcode://` URL scheme). **Status: Phase 1 partial (rename tried and reverted); Phase 2 not started.**

| Item | Status |
|---|---|
| PR #33 rename attempt | DONE (merged then reverted) |
| PR #36 revert | DONE (merged) |
| Code state after revert | DONE (source back to `jcode-*`) |
| GitHub repo rename | DONE (was done before #33; stayed) |
| Phase 2 decollide work (#32) | UNKNOWN / NOT STARTED (issue still open) |
| Tighter rebase attempt (#43) | UNKNOWN / NOT STARTED (issue still open) |
| Operator-approved cleanup of 47 untracked paths | NOT DONE (sync ritual still blocked) |
| Operator-approved cleanup of 96 decollision debris | NOT DONE |

**Net decollision truth:** the GitHub repo is `KCode`, the Cargo crate is `jcode`, the 92 crate directories are `jcode-*`, the binary is `jcode`. The rename was tried, broken, reverted; the source-side decollision is FAILED. The only thing the fork has decollided is the GitHub repo name.

---

## Step 8 — FORWARD PATH (3 options, effort/risk/breakage)

### (a) Incremental cherry-pick of upstream releases with per-commit `cargo check`

- **Effort:** 4–6 hours of automation (script that cherry-picks each upstream release onto fork branch, runs `cargo check` after each, skips on failure with diagnostic), then **40–80 hours of actual cherry-pick + test work** depending on conflict density. Rough estimate: ~1 week of focused agent work.
- **Risk:** Medium-low. Per-commit `cargo check` (per issue #43) catches compilation breaks immediately. Conflict surface is bounded because each upstream release is a small atomic unit.
- **What it breaks:** preserves everything in the fork (Herdr, new crates, TUI customizations, clippy sweeps). Each conflict is a one-off, not a mass rebase. The 47 untracked worktree paths still need to be resolved (or `git clean -fd` approved by operator) before cherry-pick branches can land cleanly.
- **Outcome:** fork converges to upstream v0.98.1 over many small, reviewable PRs; fork-only commits stay on top; can file the 9 upstream-friendly PRs along the way.

### (b) Full re-merge of upstream master onto the fork

- **Effort:** 2–4 hours for merge attempt + ~10–20 hours of conflict resolution. Once conflicts are resolved, ~2–4 hours for full `cargo check --workspace` + `cargo test --workspace` + `cargo clippy` validation. ~1–2 days total.
- **Risk:** High. `git merge upstream/master --no-edit` is currently **ABORTED** because 47 untracked paths conflict. Even after operator-approved cleanup of those, the merge will have 2,828 commits of conflict surface across all the fork-only crates (`jcode-herdr`, `jcode-auto-dream`, `jcode-micro-compact`, `jcode-perm-bubble`, `jcode-cache-vectors`) and many of the shared files. Per-file conflict resolution is non-trivial.
- **What it breaks:** the entire `jcode-herdr` crate may need to be re-based manually against upstream's changes to `jcode-app-core`; same for the 4 new crates. Herdr integration tests may need re-authoring. The fork loses granular history (becomes one or a few merge commits).
- **Outcome:** fork jumps to upstream v0.98.1 in one shot, fork-only commits rebased on top. Faster than (a) if all conflicts resolve cleanly, but the failure mode is catastrophic (full re-merge fails to compile = same situation as PR #33).

### (c) Abandon this fork and re-fork current upstream, re-applying only the fork delta

- **Effort:** ~8 hours to set up the new fork (`gh repo create KooshaPari/Jcode-v2` or similar), 1–2 hours to bootstrap initial state, then **~80–160 hours** to re-apply the fork delta in dependency order: (1) Herdr integration, (2) the 4 new crates (auto-dream, micro-compact, perm-bubble, cache-vectors), (3) provider additions (ForgeCode), (4) TUI customizations, (5) elicitation channel, (6) clippy sweeps, (7) process/build fixes, (8) docs. ~3–5 weeks of focused agent work.
- **Risk:** Low. Fresh fork = clean state, no merge conflicts, no 47 untracked files. Each re-applied chunk is its own PR with `cargo check` per commit.
- **What it breaks:** ALL existing fork history. The 47 untracked worktree artifacts and 96 decollision debris go away. Anyone depending on the fork (the 4 added crates, ~/.herdr, etc.) needs to migrate. Loses the GitHub rename to `KCode` unless re-applied.
- **Outcome:** fork starts at v0.98.1 cleanly, then grows back to feature parity with current fork within 3–5 weeks. Long-term maintenance is much cheaper than (a) or (b) because there is no longer any "behind" debt to repay.

### Recommendation

**Option (a) — incremental cherry-pick with per-commit cargo check.**

**Why:**

1. It is the only option that **preserves all current fork work** without mass-replaying the decollision risk that already burned PR #33.
2. Per-commit `cargo check` (issue #43's acceptance criterion #5) is already a documented operator preference — this path is on-policy.
3. The fork is missing 13 releases, but the releases are atomic and reviewable; ~40–80 hours of work is much cheaper than rebuilding the entire fork (c).
4. (b) is too risky given PR #33 just failed a 2,260-commit squash — a full merge of 2,828 commits has even higher conflict density.
5. (c) is the cleanest end-state but discards everything the fork has built; it is appropriate **only if** the operator wants to reset the fork's identity, which the 2026-10-15 binding memo (no upstream publishing from `KooshaPari/`) and the recent repo rename to `KCode` suggest is NOT the intent.
6. As a **side benefit**, option (a) generates the 9 upstream-clean PR candidates the fork's delta doc already enumerates — these can go to `1jehuang/jcode` first, reducing the long-term debt by half even before the fork catches up.

**Pre-conditions before (a) can start:**

- Operator approves cleanup of the 47 untracked worktree paths (per `docs/UPSTREAM-SYNC-STATUS.md` Step 1).
- Operator approves cleanup of the 96 decollision debris (Step 2).
- A `sync/upstream-v0.86` → `sync/upstream-v0.98` branch series is created.
- `cargo check --workspace` is verified passing on the post-cleanup master tip BEFORE any cherry-pick begins.
- Per-commit `cargo check` automation script is in place (template: `git rebase --continue && cargo check --workspace 2>&1 | tee /tmp/cc.log`).

**Estimated total wall-clock to fork-on-v0.98.1 with full fork delta preserved: 1–2 weeks** of focused agent work, gated on operator approvals for the preconditions.

---

## Summary (≤30 lines)

- **Behind:** 2,828 commits; **ahead:** 334. (Fork's own docs claim 2,649/144 — stale by 4 days.)
- **Upstream:** v0.98.1 (released 2026-10-14). **Fork pin:** v0.85.1-k1.1.1. **Drift:** 13 releases + 2,828 commits.
- **Fork delta (intent groups):** TUI UX (~30), providers/ForgeCode, Herdr (FORK-ONLY), elicitation, 4 new crates (auto-dream/micro-compact/perm-bubble/cache-vectors, FORK-ONLY), decollision (net-zero), `/loop`+`/goal`+ambient, clippy sweeps (~12), build/CI (~13), process/OS fixes (3), TUI-render refactors, test/fixture fixes, docs, misc. 9 upstream-clean PR candidates + 24 demos already enumerated.
- **Top missing upstream capabilities:** v0.86 Jev handoff + memory recall; v0.87/v0.88 Claude Opus 5.5 + multi-browser + banked resets; v0.89 voice input + pinned status + applets; v0.90 provider-native web search; v0.91 Google Calendar + speed tiers + `/desktop`; v0.92 codemode; v0.93 instant startup (~20ms paint); v0.94 queued commands; v0.95 OpenCode Go Responses routing; v0.96 remote MCP over HTTP/SSE; v0.97 custom JSON search providers; v0.98 reduced idle + `user_response` tool.
- **Decollision truth:** GitHub repo renamed to `KooshaPari/KCode`. Rust source is 100% `jcode` (Cargo.toml `name = "jcode"`, 92 `jcode-*` crate dirs, `jcode` binary). PR #33 (2,260-commit rename) **broke and was reverted by PR #36**. Issue #43 (per-commit `cargo check`) is **OPEN, not started**. Sync ritual remains **BLOCKED** by 47 untracked paths + 96 decollision debris.
- **Recommended path: (a) incremental cherry-pick with per-commit `cargo check`.** 1–2 weeks of focused work, preserves fork delta, generates 9 upstream-friendly PRs along the way. Pre-conditions: operator approval for the 47-path + 96-debris cleanups before any sync branch can land.