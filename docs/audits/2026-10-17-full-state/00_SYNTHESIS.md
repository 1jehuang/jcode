# Cross-Audit Synthesis — KooshaPari/KCode

**Date:** 2026-10-16
**Scope:** Five read-only audit lanes (A–E) of `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode` (the working tree of `KooshaPari/KCode`, formerly `KooshaPari/Jcode`).
**Method:** Each lane ran independently; this document reconciles findings and surfaces the 5 cross-audit decisions worth the operator's attention.
**Source files:**
- `~/.jcode/scratch/audit/A-stubs-mocks.md` (218 lines)
- `~/.jcode/scratch/audit/B-test-integrity.md` (452 lines)
- `~/.jcode/scratch/audit/C-goals-reality.md` (266 lines)
- `~/.jcode/scratch/audit/D-fork-delta.md` (223 lines)
- `~/.jcode/scratch/audit/E-usability.md` (351 lines)

---

## One-line per lane

| Lane | Topic | TL;DR |
|---|---|---|
| **A** | Stubs / mocks / TODOs in prod | Repo is **unusually clean** (0 `todo!`, 0 `unimplemented!`, 0 `panic!` unimplemented). The only **real, user-facing stubs** are: (1) swarm rename UI lies "Rename sent" but never sends; (2) auto-dream feature logs "would consolidate" but never calls the LLM. Plus ~5 "kept for future use" `#[allow(dead_code)]` items. **~3-4 days of focused work** clears all real findings. |
| **B** | Test integrity | **9,624 tests** in the tree, **80 ignored** (62 with reasons, 18 bare). **~80% of crate lib-tests (75 of 92 crates) are NEVER run by CI** — CI explicitly names only `app-core`, `base`, `embedding`, `harness-api`, `sdk`, `setup-hints`, `tui`. 26 ignored tests are real-path live coverage (live API, GUI, network) that developers never see. The `tests/e2e/` tree is mostly `MockProvider` (pre-scripted events), not real providers. **No systematic empty-body / tautology problem.** |
| **C** | Goals vs reality | Of 16 goal-bearing docs: **2 DONE, 9 PARTIAL, 5 NOT STARTED, 0 ABANDONED, 0 FALSE-DONE.** The **Modular Architecture RFC** is the biggest gap (92 crates exist, but root still owns ~21K lines of CLI). **Memory Graph Plan** is the biggest win (recall@5: 0.0→0.75 via hybrid+rerank detour). 7 of 12 open PRs are stale drafts. |
| **D** | Fork vs upstream delta | Fork is **2,826 commits behind `1jehuang/jcode/master`, 335 ahead**. Ahead = Herdr (~11 commits), 4 new crates (auto-dream, micro-compact, perm-bubble, cache-vectors), TUI polish, provider additions, elicitation channel, clippy sweeps. **Decollision rename is fully reverted in code** (PR #33 squash-merged, PR #36 reverted byte-for-byte); only the GitHub repo name `KCode` remains. Fork is missing 13 upstream releases (v0.86.0–v0.98.1) covering voice input, calendar, codemode, queued commands, remote MCP, browser-bridge updates, Windows companion. **Recommended forward path: incremental cherry-pick with per-commit `cargo check`** (~1–2 weeks, preserves all fork work, generates upstream-clean PRs along the way). |
| **E** | Usability | Brand-new user with any major-provider credential: **install in <60s, productive in next minute**. No credentials → beautiful onboarding flow stops at login prompt (no demo mode). **Biggest adoption gaps:** no demo mode, fragmented auth-state locations, telemetry-on-by-default during install, `cargo build` ≠ `jcode self-dev --build` gotcha for contributors. README's "40-provider" claim conflates "wired via openai-compatible" with "native integration" — the real count is 11 dedicated provider crates. |

---

## The 5 cross-audit decisions worth the operator's attention

### Decision 1 — Fix the two user-facing silent-failure stubs (A-1, A-2)
- **What:** (1) Wire `SwarmCommand::RenameAgent` in `crates/jcode-tui/src/tui/app/tui_state.rs:2326` so the UI's "Rename sent" status notice is true. (2) Either implement the LLM call in `crates/jcode-auto-dream/src/lib.rs:99-110` or rename `DreamOutcome::WouldConsolidate` to something honest like `GatesSatisfiedNoLlmCall`.
- **Why first:** Both A-1 and A-2 are **silent data-loss / silent-failure modes** that a paying user would hit within minutes. Everything else is debt; these are bugs.
- **Effort:** 4-6 hours (rename wiring) + 1-2 days (auto-dream LLM call). Lane A is otherwise clean.
- **Pre-conditions:** None.

### Decision 2 — Pick the forward path on the fork (D-3)
- **What:** The fork is 13 releases behind upstream. The recommendation in D is **option (a): incremental cherry-pick of upstream releases with per-commit `cargo check`**. This preserves all 335 fork-ahead commits (Herdr, new crates, TUI polish) without replaying the PR #33 / PR #36 decollision disaster.
- **Why now:** Every week of delay adds upstream drift; the 9 fork-ahead commits that ARE upstream-compatible (elicitation channel, OpenCode Go responses routing, some clippy sweeps) could be filed to `1jehuang/jcode` immediately and cut the long-term debt in half.
- **Pre-conditions before cherry-pick can start:** Operator must approve cleanup of 47 untracked worktree paths and 96 decollision debris files (currently blocks `git merge upstream/master`).
- **Effort:** 1-2 weeks of focused agent work, gated on operator pre-conditions.

### Decision 3 — Expand CI test surface (B-7b)
- **What:** Of 92 workspace crates, **75 have `#[test]` functions CI never invokes** (only `app-core`, `base`, `embedding`, `harness-api`, `sdk`, `setup-hints`, `tui` are explicitly named in `.github/workflows/ci.yml::build`). This is the largest single gap between the project's test surface and what is actually validated on every commit.
- **Options:**
  - (a) Add a meta-crate-level "compile + run unit tests for every lib" job to CI. Modest cost (~10-20 min additional per build), catches enormous surface.
  - (b) Stay narrow but add the 3-5 most-load-bearing crates (jcode-provider-core 127 tests, jcode-provider-openai-runtime 136, jcode-provider-openrouter-runtime 135, jcode-tui-markdown 148, jcode-command-risk 209).
  - (c) Accept current scope, document it. Status quo.
- **Why not first:** lane A is sharper (silent failures) and lane D's fork-sync is time-sensitive. This is the third priority.

### Decision 4 — Triage stale open PRs (C-4)
- **What:** 7 of 12 open PRs are stale drafts from late September (macOS trust, ForgeCode provider, ACP, server identity). PR #23 (v0.88.0 merge) sits in unknown state since October 1. PR #35 has merge conflicts. Only PR #58 (Windows transport) shows active development.
- **Action:** Either push these to merge-or-close, or assign each to an owner and a deadline. The repo has 23 open issues; many are likely blocked by these stale PRs.
- **Why:** Open PR rot is corrosive — every future contributor wastes time triaging them.

### Decision 5 — Define the Modular Architecture RFC's actual scope (C-2)
- **What:** The Modular Architecture RFC envisions the root crate as a composition shell with server/agent/TUI extracted. The reality is 92 crates exist as a **data-contract shell around a monolithic root** (root still owns ~21K lines of CLI; `src/cli/commands.rs` is 3,482 lines, `src/cli/acp.rs` is 2,194 lines). The plan was 7 phases; the audit finds **none of the acceptance criteria met**.
- **Options:**
  - (a) **Revise the RFC** to match reality: keep the workspace as data-contract shell, document the root as a stable monolith-by-design, stop treating the modular extraction as in-flight.
  - (b) **Re-commit to the RFC** as the post-fork-sync work (after Decision 2): once on upstream v0.98, do the extraction in a series of per-phase PRs.
  - (c) **Abandon the RFC** silently: keep the 92 crates (they help compile times) but stop claiming modularity.
- **Why not first:** depends on D's fork-sync decision; will be moot if (b) is the chosen scope.

---

## Cross-audit cross-references (where lanes touch)

| Cross-ref | Lanes | Finding |
|---|---|---|
| `jcode-auto-dream` is a stub | A-2, C-3 | Memory Graph Plan claims auto-dream is "shipped"; the actual LLM call is a TODO. The plan's own evidence marks auto-dream as deferred, but the crate is wired into `jcode-app-core` and would gate on the LLM call the moment its trigger conditions fire. **Either implement the LLM call or remove the crate from the production wiring.** |
| Herdr is untracked worktree debris | A-1, D-5 | 47 untracked worktree paths include `crates/jcode-herdr` and 4 sibling new crates. Until operator approves `git clean -fd` (or selective `git add`), cherry-pick branches in D will conflict. |
| 80% of crate tests never run in CI | B-7b, C-1 | CODE_QUALITY_10_10_PLAN.md Phase 0 is "all done" per C; but the test-coverage expansion that should have followed it didn't. The phase is reported DONE; the substance isn't. |
| 23 open issues + 7 stale PRs | C-4, C-1 | Two of the 5 NOT-STARTED docs (manager mode, screen-capture security) have open issues tracking them; the stale PRs may be the implementation attempts. Triage should connect issues → PRs. |
| README's 40-provider claim | E, C | E flags the 40-vs-11 mismatch as a usability gap (sets false expectations). C finds 0 of the goal-bearing docs make this claim — it lives only in README. Easy fix: edit README to 11 + `openai-compatible` count, or list the openai-compatible names. |

---

## Action menu for the operator

| # | Action | Depends on | Effort | Risk |
|---|---|---|---|---|
| **1** | Fix swarm-rename + auto-dream stubs (A-1, A-2) | nothing | 1-2 days | very low |
| **2** | Approve worktree cleanup + start upstream cherry-pick (D-3) | operator cleanup approval | 1-2 weeks | low (per-commit cargo check) |
| **3** | File 9 upstream-clean PRs from existing fork delta (D-5) | nothing | 2-3 days | very low |
| **4** | Triage 12 open PRs (C-4) | nothing | 1-2 hours | very low |
| **5** | Expand CI to cover more lib-test surfaces (B-7b) | nothing | 1-2 days for YAML, +20 min/build | low |
| **6** | Decide Modular Architecture RFC scope (C-2) | D-3 outcome | 1-2 days to revise or re-commit | medium |
| **7** | Edit README's provider-count claim (E) | nothing | 30 min | very low |

**Suggested order:** 4 → 1 → 3 → 2 → 5 → 6 → 7. (Triage the PR rot first, then fix the silent failures, then capture the upstream-clean PRs, then the heavy fork-sync, then CI, then the long-arc RFC decision, then the cosmetic README fix.)

---

## What this synthesis does NOT cover

- Lane-by-lane deep dives — each audit file is the canonical record (200-450 lines, read directly).
- Per-file change lists — A has a top-10 ranked table; D has the full upstream-deficit list (v0.86→v0.98.1). Read those files for the implementation-level detail.
- PR review of the 12 open PRs — that's lane C's "next step" recommendation, not synthesized here.
- Per-crate CI test count — lane B's section 7b has the full table.
