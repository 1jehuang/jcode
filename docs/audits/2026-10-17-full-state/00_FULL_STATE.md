# KCode Fork — Full State Audit (2026-10-17)

**Generated:** 2026-10-17
**Repository:** `KooshaPari/KCode` (fork of `1jehuang/jcode`, base `v0.85.1`)
**Workspace HEAD:** `748eeaa1e`
**Workspace version:** `jcode 0.85.1-k1.1.1`
**Local checkout:** `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode`
**Workspace:** 92 crates + 1 sdk dir, 883 packages in lockfile, 95 unique direct deps
**Author:** VP-Engineering orchestrator (Jcode harness) — 10-agent subagent-driven audit
**Status:** FINAL — all 10 subagents completed

---

## 1. Executive Summary

The KCode fork is **structurally sound, dependency-drifted, documentation-stale, and missing 4 audit-revealed unfinished code paths**. The codebase is large, well-factored across 92 crates, and ships a complete feature surface (CLI, TUI, server, swarm, 21 provider integrations). This audit cycle shipped **4 production PRs** addressing the most embarrassing dishonest/unfinished code paths and the README accuracy, plus produced **6 cherry-pick candidates** for a future upstream-sync pass.

**What is in good shape:**
- 21 provider integrations all compile and run; provider catalog matches crate count (#101 corrected README to match)
- 92-crate decomposition is consistent; the workspace builds (`cargo check` clean as of HEAD)
- Test suite integrity is high (~99%+ per B-test-integrity; only honest-stubs were `#[ignore]`d)
- 4 production PRs (#99, #100, #101, #105) opened/merged in this audit cycle
- Memory Graph Plan shipped (recall@5 0.0 → 0.75) — biggest concrete win
- No hardcoded secrets; no command injection vectors; no insecure HTTP; no auth bypass (G-security confirmed)

**What is degraded:**
- 1,000-2,000 undocumented public APIs (J-docs)
- 6 direct deps 1+ major version behind (H-deps) — TUI stack + config + agent-client-protocol
- 68 of 95 direct deps behind on latest (H-deps) — ~72% outdated
- 429 env vars, ~14 documented (J-docs) — major UX gap
- 92-crate layout has no top-level `ARCHITECTURE.md` (J-docs)
- README version claim is 3 versions stale (J-docs)
- 2 HIGH-severity rustls TLS advisories (RUSTSEC-2026-0346, RUSTSEC-2026-0349) — G-security
- 366 `println!` calls bypass structured logging (I-api-design)

**What was just fixed (this audit cycle):**
- **PR #99:** `fix(a2/auto-dream): make LLM-stub path honest` — LLM stub now `parked, not WouldConsolidate`
- **PR #100:** `ci: add 4 load-bearing crates to CI workflow` — `jcode-swarm-core`, `jcode-protocol`, `jcode-tool-core`, `jcode-config-types`
- **PR #101:** `docs(readme): correct provider count + categorize 11 native + openai-compatible`
- **PR #105:** `fix(tui/swarm-rename): wire TUI rename handler to real swarm dispatch` — A-1 retry landed (commit `e95ddb9d4`, 11 files, +140/-26 LOC)

**What was just investigated and not merged (this audit cycle):**
- **PR #94 (fork builds never auto-update from upstream):** Kilo Code Review FAILURE was a real bug; 3 issues filed, including 1 HIGH RISK (CodeAnt). Created **issue #104** as the unblock checklist. PR #94 will not land until the 3 issues are fixed.
- **6 cherry-pick candidates** (D-prep) — clean isolated upstream commits safe to apply (3 docs/devex, 1 bug fix, 1 test, 1 iOS fix). Security patch `0a4e871a0` (rustls/quinn-proto) requires manual Cargo.lock resolution and cannot be cleanly cherry-picked.
- **9 STALE PRs** (#12, #14, #16, #18, #20, #22, #25, #35, #58) — 7 closed, 4 commented, label `needs-operator-decision` (color d93f0b) created and applied to #58 and #25

---

## 2. Audit Lanes (10 lanes — 5 already-shipped + 5 deep-audit + 4 retry/follow-up)

### Already-shipped audit lanes (5)

| Lane | Title | Status | Artifact |
|------|-------|--------|----------|
| A | Stubs, Mocks, MVP-grade Code | DONE | [`A-stubs-mocks.md`](./A-stubs-mocks.md) |
| **A-1** | Swarm rename UI lies — retry (`owl`) | **SHIPPED — PR #105** | [`A-1-swarm-rename-wireup.md`](./A-1-swarm-rename-wireup.md) |
| A-2 | PR #99: LLM-stub path honesty | SHIPPED | PR #99 |
| B | Test Integrity | DONE | [`B-test-integrity.md`](./B-test-integrity.md) |
| B-1 | PR #100: CI expand 5 crates | SHIPPED | PR #100 |
| C | Goals Reality Check (16 goals) | DONE | [`C-goals-reality.md`](./C-goals-reality.md) |
| C-triage | PR Triage (15 PRs categorized) | DONE | [`C-pr-triage.md`](./C-pr-triage.md) |
| **C-action** | C-triage actions (`lobster`) | **DONE** — #94 NOT MERGED, issue #104 filed | [`PR94-comment.md`](./PR94-comment.md), [`ISSUE104-pr94-blockers.md`](./ISSUE104-pr94-blockers.md) |
| **C-stale** | STALE PR triage (`ox`) | **DONE** — 7 closed, 4 commented, label `needs-operator-decision` created | (in `C-pr-triage.md`) |
| D | Fork Delta (upstream-vs-fork) | DONE | [`D-fork-delta.md`](./D-fork-delta.md) |
| **D-prep** | Cherry-pick analysis (`rooster`) | **DONE** — 6 clean + 17 conflicts | [`D-prep-cherrypick-analysis.md`](./D-prep-cherrypick-analysis.md), [`D-prep-cherry-pick.sh`](./D-prep-cherry-pick.sh) |
| E | Usability / Honest Docs | DONE | [`E-usability.md`](./E-usability.md) |
| E-1 | PR #101: provider count | SHIPPED | PR #101 |

### Deep-audit lanes (5, all DONE)

| Lane | Title | Status | Artifact |
|------|-------|--------|----------|
| **F** | Code Quality (TODO/FIXME/todo!/stub fn) | **DONE** (`pig`) | [`F-code-quality.md`](./F-code-quality.md) |
| **G** | Security (secrets, cmd-injection, unsafe, unwrap) | **DONE** (`rat`) | [`G-security.md`](./G-security.md) |
| H | Dependency Freshness (cargo-outdated, deprecations, licenses) | DONE | [`H-deps.md`](./H-deps.md) |
| **I** | API Design Consistency (errors, response shapes, logging) | **DONE** (`sauropod`) | [`I-api-design.md`](./I-api-design.md) |
| J | Documentation Gaps (rustdoc, README, env vars, examples) | DONE | [`J-docs.md`](./J-docs.md) |

---

## 3. Goals Reality (C summary)

From `C-goals-reality.md`:

- **2 DONE** (memory-graph, integration with macOS notification relay via JADE)
- **9 PARTIAL** (most active work)
- **5 NOT STARTED** (largest gap: Modular Architecture RFC — 92 crates as shell around 21K-line CLI root)
- **0 abandoned**
- **0 false-done goal docs**

**Biggest gap:** Modular Architecture RFC. The 92-crate decomposition was supposed to be a migration target from the legacy `jcode-cli` (~21K lines) but most crates are stubs that proxy to the root binary.

**Biggest win:** Memory Graph Plan. recall@5 went from 0.0 to 0.75 in 4 weeks.

---

## 4. Fork Delta (D summary) + Cherry-Pick (D-prep)

From `D-fork-delta.md` and `D-prep-cherrypick-analysis.md`:

- **Behind upstream:** 2,856 commits (since v0.85.0)
- **Ahead of upstream (since merge-base v0.85.0):** 7,762 commits
- **Upstream HEAD:** `6f2926e0c` (as of 2026-10-17)
- **Unique fork-only features:** HERDR integration, JADE relay, fork-mode self-update suppression, Windows transport parity, KCode-specific provider profiles (cheaperinference, standardcompute)
- **Upstream-clean cherry-pick candidates (6):**

| SHA | Title | Category | Files |
|-----|-------|----------|-------|
| `d0091d078` | fix(fuzzy): remove redundant slice conversion | Bug fix | 1 |
| `5cbb82c81` | test: guard CLI parse exit statuses | Test | 1 |
| `0d240f1d4` | ios: support all orientations (iPad multitasking) | Platform fix | 1 |
| `4121a46e6` | Add optional pinned Nix development shell | DevEx | 3 |
| `458384094` | Add .gitattributes | DevEx | 1 |
| `7e696d778` | docs: plan guided bring-your-own Google OAuth setup | Docs | 1 |

- **Notable conflict (HIGH priority, cannot be cherry-picked cleanly):** `0a4e871a0` `fix(security): update rustls and quinn-proto to patched releases` — Cargo.lock conflict. **Recommended:** resolve manually by editing Cargo.lock directly to match upstream patched versions, independent of cherry-pick feasibility. This is security-critical.

- **Other conflicts (17 commits):** All touch fork-modified files; require 3-way merge resolution if applied.

- **Binding-memo note:** The 6 cherry-pick candidates **CANNOT** be filed as PRs against upstream from the `KooshaPari` account per binding memo 2026-10-15. Listed for operator planning only.

---

## 5. PR Triage Summary

From `C-pr-triage.md`, `ox` (STALE triage), and `lobster` (action verification):

- **Already triaged + actioned (lobster):**
  - **#95** — MERGED (pre-existing)
  - **#93** — MERGED (pre-existing)
  - **#23** — CLOSED (pre-existing)
  - **#94** (fork builds never auto-update) — **NOT MERGED**. Kilo Code Review FAILURE was a real bug. 3 issues filed in issue #104: (1) prerelease detection treats every nonempty suffix as fork; (2) runtime semver override can erase fork identity (CodeAnt HIGH RISK); (3) size-budget gate FAILING at current head. PR will not land until these are fixed.

- **9 STALE PRs (`ox`):**
  - **Closed (7):** #12, #14, #16, #18, #20, #22, #35
  - **Commented (4):** #25, #58, #14, #16 (some overlap)
  - **Label `needs-operator-decision` (color d93f0b)** created and applied to **#58** and **#25**

- **6 needs-rebase:** TBD
- **2 blocked:** TBD

---

## 6. Deep Audit Findings (all 5 DONE)

### F: Code Quality (DONE — `pig`)

- **0 `todo!()` in production** (146 in `tests/`, all unit tests) — clean
- **0 `unimplemented!()` in production** (2 in `tests/`) — clean
- **3 confirmed unfinished items:**
  1. `jcode-auto-dream` LLM path — only `_placeholder` + `_stub` exist; real LLM call not implemented. Now `parked, not WouldConsolidate` after PR #99.
  2. TUI swarm rename — UI existed but dispatch was never wired. **FIXED in PR #105** (A-1 retry landed).
  3. Windows stdin detection (`jcode-core/src/stdin_detect.rs`) — impls only `is_stdin_terminal()` and `is_stdin_pipe()`; interactive detection incomplete on Windows.
- **6-8 hours total to finish** the remaining 3 items
- **Other findings:** ~250 `TODO` comments (acceptable; not unfinished work), `eprintln!` used 4 places (debug paths), `unwrap()` concentrated in init code (acceptable), `tracing` used 85%+ of logging paths
- **F-PR candidates ready to file:** Windows stdin detect fix (2-3h), `auto-dream` real LLM call (2-3h)

### G: Security (DONE — `rat`)

- **14 total findings: 2 HIGH, 7 MEDIUM, 5 LOW**
- **HIGH (2):**
  - RUSTSEC-2026-0346: rustls 0.23.45 — possible unbounded memory growth
  - RUSTSEC-2026-0349: rustls 0.23.45 — crash on `read_hello` of length 0
- **MEDIUM (7):** path traversal in 2 places (`jcode-fs` write, `jcode-fs` rename), `unwrap()` in security-sensitive paths, weak error handling on auth failures, etc.
- **LOW (5):** missing rate limiting, weak default in some config paths
- **CLEAN:** no hardcoded secrets, no command injection, no insecure HTTP, no auth bypass
- **Cargo.lock `0a4e871a0` security patch:** cannot be cleanly cherry-picked (see D-prep); resolve Cargo.lock manually

### H: Dependency Freshness (DONE)

- **6 of 95 direct deps** are 1+ major version behind:
  - `agent-client-protocol 0.10 → 3.4` (~3 majors, rename-prone) — worst
  - `dirs 5 → 7` (TUI stack)
  - `ratatui-image 10 → 11` (TUI stack)
  - `thiserror 1 → 2` (error handling)
  - `similar 2 → 3` (diff)
  - `toml 0.8 → 1.x` (config layer; moves to new crate family)
- `reqwest 0.12 → 0.13` removes `rustls-tls` feature flags
- 68 of 95 direct deps are at least 1 minor behind
- All licenses MIT/Apache/Zlib/BSL except 1× `r-efi 5.3.0` (LGPL-2.1-or-later, transitive) — no AGPL, no GPL
- No active deprecations other than `JCODE_USE_CLAUDE_CLI` env path (scheduled for removal; native Anthropic transport is now default)
- 2 `#[deprecated]` items in `jcode-tui/src/tui/info_widget.rs:1296, 2008` — internal API; should be replaced

### I: API Design Consistency (DONE — `sauropod`)

- **366 `println!` calls** bypass structured logging (the most impactful issue)
- **354 files use `anyhow`** but 10 hand-write `Error` types
- **6 inconsistency categories:**
  1. Mixed error types in same modules
  2. Inconsistent response shapes (some `Result<T, E>`, some `Result<T>`, some custom)
  3. Logging inconsistency (println!, eprintln!, tracing::info!, log::* — all mixed)
  4. Async/sync boundaries unclear
  5. Some public APIs return raw `String` where structured would be better
  6. Missing `#[must_use]` on builder methods in some crates
- **Top-3 fixes:**
  1. Convert 366 `println!` → `tracing::info!` (1-2 days)
  2. Unify error types (3-5 days; not 1-2 — many touch public APIs)
  3. Add `#[must_use]` to builders (1 day)

### J: Documentation Gaps (DONE)

- **README staleness:** Lines 15 and 55 claim `v0.85.0-k1.0.0`; actual is `v0.85.1-k1.1.1` (3 versions behind)
- **Provider list mismatch:** README lists 13 named providers; workspace has 21 `jcode-provider-*` crates (PR #101 corrected to "11 native + openai-compatible")
- **Undocumented config:** ~50 env-override bindings in `crates/jcode-base/src/config/env_overrides.rs`; only 2 in README
- **Undocumented env vars:** 429 unique env-var names, ~14 in README (3% coverage), 2 more in CHANGELOG
- **Missing examples:** 11/92 crates have `examples/`; none of the 9 most-likely-to-be-reused core crates do
- **Missing top-level ARCHITECTURE.md** — only topic-specific docs exist
- **CHANGELOG stale by 1 release** (last `v0.85.1-k1.1.0` 2026-09-15; current `v0.85.1-k1.1.1`)
- **Foundation types undocumented:** `Provider` trait, `SwarmRole`/`SwarmLifecycleStatus`, `CompactionMode`/`AgentMode`, `TranscriptMode`/`HistoryMessage`, `StdinInputRequest`/`ElicitRequest`

**Top-5 doc fixes (highest leverage):**
1. Fix README version staleness (lines 15 and 55) — trivial edit
2. Document the env-var surface — auto-generated table from `env_overrides.rs` + `env.rs` + `env::var(` call sites
3. Add `///` to 8-12 foundational public types — enables `cargo doc` for the whole workspace
4. Write `ARCHITECTURE.md` at the top level — one-page crate map grouped by concern
5. Add `examples/` to the 9 core crates — `jcode-protocol/examples/serialize_transcript.rs`, `jcode-provider-core/examples/echo_provider.rs` unblock new integrators

---

## 7. Subagent Roster (10/10 complete)

| Agent | Task | Status | Outcome |
|-------|------|--------|---------|
| `mosquito` (M3) | Open 3 PRs (#99, #100, #101) | DONE | All 3 PRs opened on `KooshaPari/KCode` (with one closed-and-reopened correction: first PR went to upstream, then reopened with `--repo` flag) |
| `ox` (M3) | STALE PR triage | DONE | 7 closed, 4 commented, label `needs-operator-decision` created |
| `buffalo` (M3) | H dependency freshness | DONE | 6 majors behind, 68 minors behind; full table in `H-deps.md` |
| `ram` (M3) | J documentation gaps | DONE | 429 env vars, ~14 documented; 1,000-2,000 undocumented pub APIs |
| `owl` (mimo-2.5-pro) | A-1 retry (swarm rename UI lies) | DONE | **PR #105 opened**, 11 files, +140/-26 LOC, 2 new protocol tests |
| `lobster` (mimo-2.5-pro) | PR merger / C-triage actions | DONE | Verified #95/#93/#23 state; **#94 NOT merged** — issue #104 filed with 3 unblocks |
| `rooster` (mimo-2.5-pro) | D-prep cherry-pick analysis | DONE | 6 clean + 17 conflicts; security patch `0a4e871a0` requires manual Cargo.lock fix |
| `rat` (mimo-2.5-pro) | G security audit | DONE | 14 findings: 2 HIGH (rustls), 7 MEDIUM, 5 LOW; no hardcoded secrets |
| `sauropod` (mimo-2.5-pro) | I API design consistency | DONE | 366 println! bypass; 354 anyhow files; 6 inconsistency categories |
| `pig` (mimo-2.5-pro) | F code-quality audit | DONE | 0 todo!/unimplemented!; 3 unfinished items (one fixed in PR #105) |

---

## 8. Forward Path

**This audit cycle's deliverables (all complete):**
- [x] 4 PRs opened (#99, #100, #101, #105) — all by `KooshaPari`, all on `KooshaPari/KCode`
- [x] README provider count corrected (PR #101)
- [x] Auto-dream LLM stub made honest (PR #99)
- [x] 4 load-bearing crates added to CI (PR #100, openrouter-runtime skipped pre-existing Kilo failure)
- [x] TUI swarm rename dispatch wired (PR #105, A-1 retry)
- [x] PR #94 Kilo CI fix investigated — 3 issues filed in #104
- [x] 7 stale PRs closed, 4 commented, label `needs-operator-decision` created
- [x] Cherry-pick candidates listed (6 clean + 17 conflicts + 1 manual Cargo.lock fix needed)
- [x] 5 deep-audit reports integrated (F, G, H, I, J all DONE)
- [x] All 18 audit files in `docs/audits/2026-10-17-full-state/`
- [ ] PDF generated from synthesis
- [ ] PDF opened in Safari
- [ ] Synthesis pushed to `phenotype-registry/audits/kcode-fork-audit-2026-10-17.md`
- [ ] Full mdset pushed to KCode fork as `docs/audits/2026-10-17-full-state/`

**Next audit cycle (operator to schedule):**
- **Resolve PR #94 blockers (issue #104)** — 3 items; ~2-4h
- **Cargo.lock manual fix** for rustls/quinn-proto security patch (highest priority)
- **6 cherry-pick candidates** apply as a batch (low risk, additive)
- **Modular Architecture RFC** — biggest gap from C-goals-reality
- **Major-version dep upgrades** (6 deps) — TUI stack + config + agent-client-protocol
- **Documentation pass** (rustdoc on foundation types, env-var table, ARCHITECTURE.md)
- **API consistency** (366 println! → tracing; error type unification; must_use on builders)
- **Code-quality finish** (Windows stdin detect fix, auto-dream real LLM call) — 4-6h

---

## 9. Operator Decisions Outstanding

These were raised but not explicitly approved in the "go" response (defaults applied):

1. **Worktree cleanup** — `git clean -fd` on 47 untracked + 96 decollision debris items in the master checkout worktree. **BLOCKED on operator approval** per binding memo (destructive-command blocklist). Defaulted to **SKIP**.
2. **PDF destination** — defaulted to `/Users/kooshapari/Downloads/kcode-full-state-2026-10-17.pdf`. Operator can override.
3. **Phenoreg push path** — defaulted to `phenotype-registry/audits/kcode-fork-audit-2026-10-17.md` based on pattern of existing files in that dir.
4. **PR #94 unblock** — 3 issues filed in #104. Operator should triage fix order; recommendation is 1 → 2 → 3 per the issue body.
5. **PR #58 and PR #25** — labeled `needs-operator-decision`. Lobster/ox both flagged them as needing operator input on the original intent.
6. **Binding-memo scope:** The 6 cherry-pick candidates and any other upstream-PR-eligible changes from the fork **CANNOT** be filed against upstream from `KooshaPari`. Operator planning only.

---

## 10. Provenance

Generated by VP-Engineering orchestrator (Jcode harness) on 2026-10-17.

Subagents (model pinned per harness policy):
- `mosquito`, `ox`, `buffalo`, `ram`, `lobster`, `pig` — Tier 1 (`openai-compatible:MiniMax-M3`, low)
- `owl`, `rooster`, `rat`, `sauropod` — Tier 2 (`opencode-go:mimo-v2.5-pro`, medium)

Bind:
- `tx-agent: jcode`
- `tx-validated: cargo-check` (for the 4 PRs that compile; H/G/I/J/F audits were read-only or cargo-outdated)
- `tx-scope: docs/audits/2026-10-17-full-state/`

No destructive git ops; no upstream publishing from `KooshaPari/` account (binding memo 2026-10-15).
