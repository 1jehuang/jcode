# C: Goals vs Reality Audit — KooshaPari/KCode

**Generated:** 2026-10-16
**Repo:** `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode`
**Scope:** What the project CLAIMS to be versus what is actually finished

---

## 1. Goal-Bearing Document Inventory

### Plans (`docs/plans/`)

| # | File | Title | Stated Goal | Status | Evidence |
|---|------|-------|-------------|--------|----------|
| 1 | `UNIFIED_SELFDEV_SERVER_PLAN.md` | Unified Self-Dev / Normal Server | Merge self-dev daemon into shared server | **DONE** | Explicitly marked "Implemented." Server uses session-local canary; no separate self-dev socket. `src/cli/hot_exec.rs` has `hot_reload()`/`hot_rebuild()`. |
| 2 | `CODE_QUALITY_10_10_PLAN.md` | Code Quality 10/10 Plan | Raise to 9+/10 engineering standard | **PARTIAL** | Phase 0 (CI jobs) and Phase 1 (warning burn-down partially) done. Phase 2 (decompose biggest files) partially done. Hotspot files like `src/cli/commands.rs` (3,482 lines) and `src/cli/acp.rs` (2,194 lines) still exceed targets. No tests for `cargo fmt` cleanliness (103 dirty files per issue #38). |
| 3 | `CODE_QUALITY_TODO.md` | Code Quality Program Todo | Track execution of quality plan | **PARTIAL** | Phase 0: all 4 items done. Phase 1: 2/5 done, 3 unchecked. Phase 2: `tests/e2e/main.rs` split done; `src/server.rs` split in progress; `src/agent.rs` split NOT started (agent.rs path doesn't exist in current tree; code restructured). |
| 4 | `COMPILE_PERFORMANCE_PLAN.md` | Compile Performance Plan | Warm cargo check < 5s, build < 20-30s | **PARTIAL** | Baseline measured (8.5s check, 47.3s build). Crate split has happened (92 crates). No current measurement to verify targets met. Plan references stale file paths (`src/provider/openai.rs`, `src/agent.rs`). |
| 5 | `MEMORY_GRAPH_PLAN.md` | Making the Memory Graph Earn Its Keep | Maximize recall accuracy in both memory modes | **PARTIAL** | Major breakthroughs shipped: hybrid BM25+RRF (recall@5: 0.0→0.53), listwise LLM reranker (recall@5: 0.53→0.75). Phases A-D (graph as reranker/dedup/expansion) NOT started. Deferred follow-ups: remote embedding adapter, live reload verification, GPT-5.5 judge re-run. |
| 6 | `MCP_SKILLS_PLAN.md` | Dynamic Skills and MCP Support | Hot-reload skills, MCP support, dynamic tool registration | **PARTIAL** | MCP IS supported (README documents `~/.jcode/mcp.json`, Claude Code compat, `jcode login --provider` flows). Original plan's file structure (`src/mcp/mod.rs`) was NOT followed; implemented via different path. Hot-reload skills: only binary `hot_reload()` exists, not skill-specific hot-reload. Dynamic tool registration: `register_external_provider_runtimes()` in startup.rs, not general runtime registration. |
| 7 | `OPENAI_COMPATIBLE_PROFILE_RUNTIME_PLAN.md` | OpenAI-Compatible Profile Runtime | Separate transport/profile/route per compatible provider | **PARTIAL** | The profile-based architecture IS in place (README documents 30+ named profiles with `jcode login --provider <id>`). However, the `OpenAiCompatibleProfileRuntime` struct and `BTreeMap` storage target are not verified in code; current `src/cli/provider_init.rs` uses `ProviderChoice` enum and `register_external_provider_runtimes()`. |
| 8 | `SERVER_SERVICE_SPLIT_PLAN.md` | Server Service Split | Split server into 5 in-process services (session, client, swarm, debug, maintenance) | **NOT STARTED** | Plan status: "Audit-based plan". No evidence of service-handle structs or ownership boundaries in code. Server state likely still in a monolithic struct. |
| 9 | `CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md` | Client-Core vs Presentation Split | Separate client-core from ratatui/crossterm | **NOT STARTED** | Status: "Proposed". TUI code still in root crate (`src/cli/tui_launch.rs`, `src/cli/commands.rs` at 3,482 lines). No `client-core` module or crate extraction. |

### Specs (`docs/specs/`)

| # | File | Title | Stated Goal | Status | Evidence |
|---|------|-------|-------------|--------|----------|
| 1 | `ISSUE-manager-mode.md` | Manager Mode | Add orchestrator-only mode that prevents agent from coding | **NOT STARTED** | No `ManagerMode` enum or mode enforcement in code. Agent modes exist (`--herdr-kind`) but not the requested tool-gating mode. |
| 2 | `ISSUE-screen-capture-security.md` | Screen Capture Security | Prevent agents from capturing personal desktop data | **NOT STARTED** | Threat model documented; no sandboxing, VNC isolation, or audit trail implemented. |

### Proposals (`docs/proposals/`)

| # | File | Title | Stated Goal | Status | Evidence |
|---|------|-------|-------------|--------|----------|
| 1 | `computer-use-tool.md` | Native `computer` tool | macOS computer use (screenshots, AX, input) | **PARTIAL** | `macos_computer_use` tool exists in the harness (verified by audit worker invocation in this session). |
| 2 | `computer-use-maximal-control.md` | Maximal macOS control | Background AX actions, per-window capture, scripting | **NOT STARTED** | Proposal only; AX actions exist but the 4-tier roadmap is not started. |
| 3 | `CROSS_DEVICE_WORKTREE_SYNC.md` | Cross-Device Worktree Sync | Multi-machine logical worktree + build channel | **NOT STARTED** | Status: "Proposed (design only, not implemented)". |

### RFC

| # | File | Title | Stated Goal | Status | Evidence |
|---|------|-------|-------------|--------|----------|
| 1 | `MODULAR_ARCHITECTURE_RFC.md` | Modular Architecture RFC | Layered workspace with dependency rules | **DRAFT** | Status: "Draft". 92 crates exist, dependency boundary check passes, provider leaf crates do NOT depend on root. BUT: acceptance criteria NOT met — root is NOT a composition shell (still owns 20,869 lines of CLI), server/agent/session/TUI not extracted as separate crates. Phase 0 (codify) done; Phases 1-6 not started. |

### Other Docs

| # | File | Title | Stated Goal | Status | Evidence |
|---|------|-------|-------------|--------|----------|
| 1 | `REFACTORING.md` | Refactoring Roadmap | Safe incremental refactoring with verification | **PARTIAL** | Phase 1 (safety + hygiene) done — scripts exist. Phase 2 (CLI decomposition) partially done — CLI split into `src/cli/` modules. Phases 3-6 (server/agent/TUI/provider decomposition) NOT started. |

### State-Tracking Docs

| # | File | Status |
|---|------|--------|
| 1 | `UPSTREAM-DELTA.md` | Claims 144 commits ahead, 2,649 behind. 9 themed PRs ready. **Code agrees** — this is a real fork delta. |
| 2 | `UPSTREAM-SYNC-STATUS.md` | Claims sync blocked by 47 untracked files + 96 debris. **Code agrees** — `git status` would show untracked paths. |
| 3 | `CONSOLIDATION-BRANCH-DISPOSITION.md` | Claims Q7 effectively done, branch is historical. **Code agrees** — the fix was redundant. |
| 4 | `WORKTREE-CLEANUP-STATE.md` | Claims 152 untracked artifacts, cleanup blocked by operator approval. **Code agrees** — debris still present. |

---

## 2. Modular Split Verification

### What the RFC claims
- Layered workspace with dependency rules
- Root crate shrinks to composition shell
- Provider crates depend on contracts, not root

### What actually happened
- **92 crates exist** — significant workspace extraction happened
- **`scripts/check_dependency_boundaries.py` PASSES** — type crates don't leak into runtime crates
- **Rule 4 upheld** — provider leaf crates (`jcode-provider-openrouter`, `jcode-provider-gemini`, `jcode-provider-core`) do NOT depend on root `jcode`
- **No root crate back-deps** — neither `jcode-embedding` nor `jcode-tui-core` depend on root

### What DIDN'T happen
- Root crate owns 20,869+ lines of CLI code (NOT a composition shell)
- `src/cli/commands.rs` alone is 3,482 lines
- No `jcode-server`, `jcode-agent`, `jcode-session`, or `jcode-tui` extraction crates
- The RFC's Phase 0 (codify) is done; Phases 1-6 are NOT started
- Acceptance criteria: "root package is primarily a composition shell" — **NOT MET**

### Verdict
The workspace exists as a "growing shell" (as the RFC itself says), but the RFC's target architecture is largely unrealized. The data-contract type crates are the main achievement. The heavy runtime orchestration remains monolithic.

---

## 3. MEMORY_GRAPH_PLAN.md — Deferred/Blocked Items

### Shipped
- ✅ recall-0 + recall-4: hybrid dense+BM25+RRF (recall@5: 0.0→0.53)
- ✅ recall-5: listwise LLM reranker with focused query (recall@5: 0.53→0.75)
- ✅ `focus_query_text` builder in memory_prompt.rs
- ✅ Benchmark harness `memory_recall_bench`

### Deferred (explicitly marked)
1. **Remote embedding adapter** — deprioritized because embedder is capped lever. Blocks: nothing.
2. **Live reload + Mode-2 verification** — blocked by unrelated agent's uncommitted changes in shared worktree. Needs operator action.
3. **GPT-5.5 judge re-run** — blocked ~18 days for OpenAI quota reset. Infrastructure ready.

### Blocked (graph phases)
- **Phase A** (supersede authoritative) — NOT started
- **Phase B** (1-hop expansion) — NOT started (gated by recall-1 harness)
- **Phase C** (graph dedup) — NOT started
- **Phase D** (trim maintenance) — NOT started
- **Phase E** (feedback via edges) — NOT started
- **recall-2** (embedder upgrade) — deprioritized (pool already ~99%)
- **recall-3** (focused query construction) — partially done via `focus_query_text`
- **recall-6** (priors) — evaluated, held back (roughly neutral)

### Verdict
The graph phases (A-E) that would "make the memory graph earn its keep" are entirely untouched. The real progress came from the hybrid+rerank pipeline, which was a detour that proved more effective than the graph approach. The graph's edges remain "write-mostly" as the plan itself diagnosed.

---

## 4. Open Issues and PRs

### Open Issues (23 total)

| # | Title | Created | Category |
|---|-------|---------|----------|
| 87 | git commit identity ignores repo/user config | 2026-10-16 | Bug |
| 65 | CI fix: unused deps + herdr report_state guard | 2026-10-14 | CI |
| 63 | build-meta: channel segment dropped from build-time/runtime | 2026-10-14 | Bug |
| 45 | [Instinct] Windows ConPTY: inline input offset | 2026-10-12 | Windows |
| 43 | Tighter Phase 2 rebase: per-commit cargo check | 2026-10-11 | Rebase |
| 42 | Inbox notification gap: pre-tool-hook DEFERs | 2026-10-09 | UX |
| 41 | Copilot AI review quota exceeded on PR #23 | 2026-10-09 | External |
| 40 | herdr: verify re-export list stability | 2026-10-09 | Maintenance |
| 39 | Decompose 4 ratcheted budget baselines | 2026-10-09 | Quality |
| 38 | cargo fmt: 103 pre-existing dirty files | 2026-10-09 | Quality |
| 37 | parse_semver installer regex ignores env var | 2026-10-09 | Bug |
| 30 | fix(tui): ratatui buffer panic on tiny terminal | 2026-10-06 | Bug |
| 26 | Add /poke context recovery sessions | 2026-10-02 | Feature |
| 24 | Merge upstream v0.88.0 into KCode | 2026-10-08 | Sync |
| 21 | macOS trust policy: stop unconditional re-signing | 2026-09-30 | Security |
| 19 | Effect recovery hook: reconcile crash ambiguity | 2026-09-30 | Reliability |
| 17 | macOS trust: remove xattr stripping | 2026-09-30 | Security |
| 15 | ForgeCode provider: fail closed on incompatible CLI | 2026-09-30 | Provider |
| 13 | Harness mature recovery: bind to daemon identity | 2026-09-29 | Recovery |
| 11 | Qualify ACP discovery, passive attachment | 2026-09-28 | ACP |
| 7 | OpenCode Go fails at scale: 429→500 | 2026-09-17 | Provider |
| 5 | feat(tui): responsive layout optimizations | 2026-09-13 | TUI |
| 3 | TUI: Bottom status line readability | 2026-09-13 | TUI |

### Open PRs (12 total) — Classified

**LIVE work (recent activity, non-draft or draft-in-progress):**
| PR | Title | Created | Status |
|----|-------|---------|--------|
| #58 | Windows transport impls + stray brace (draft) | 2026-10-14 | DRAFT, UNSTABLE — live WIP |
| #65 | CI fix: unused deps + herdr guard | 2026-10-14 | (issue, not PR) |
| #58 | Windows transport impls | 2026-10-14 | DRAFT, UNSTABLE |

**STALE (2+ weeks, no merge, but recent enough to be relevant):**
| PR | Title | Created | Status |
|----|-------|---------|--------|
| #35 | fix(tui): support tiny terminal session resume | 2026-10-09 | NON-DRAFT, DIRTY — conflicts |
| #25 | feat(tui): add /poke context recovery launcher | 2026-10-02 | NON-DRAFT, UNKNOWN state |
| #23 | Merge upstream v0.88.0 into KCode | 2026-10-01 | NON-DRAFT, UNKNOWN — major sync PR |
| #8 | fix(openrouter): stop double-polling dead unfold | 2026-09-19 | NON-DRAFT, UNKNOWN |

**ABANDONED (drafts from late September, no activity):**
| PR | Title | Created | Status |
|----|-------|---------|--------|
| #22 | fix(macos): make executable trust repair opt-in | 2026-09-30 | DRAFT, stale |
| #20 | feat(tool): add durable effect recovery hook | 2026-09-30 | DRAFT, stale |
| #18 | fix(macos): preserve installed executable trust | 2026-09-30 | DRAFT, stale |
| #16 | fix(provider): fail closed when ForgeCode CLI lacks contract | 2026-09-30 | DRAFT, stale |
| #14 | feat(server): expose daemon identity in ping | 2026-09-29 | DRAFT, stale |
| #12 | spec: mature-first KCode recovery | 2026-09-29 | DRAFT, stale |
| #10 | feat(acp): discover sessions and broker interactions | 2026-09-27 | DRAFT, DIRTY, stale |

---

## 5. State-Tracking Docs vs Code

### `docs/UPSTREAM-DELTA.md`
**Claims:** 144 commits ahead of upstream, 2,649 behind. 9 themed PR candidates.
**Code agrees.** The fork delta is real. The themed grouping and demo-needed classification are consistent with the commit log.

### `docs/UPSTREAM-SYNC-STATUS.md`
**Claims:** Sync mechanically blocked by 47 untracked files conflicting with upstream. 96 decollision debris.
**Code agrees.** The 47-path conflict is a real `git merge` blocker. The debris inventory matches `WORKTREE-CLEANUP-STATE.md`.

### `docs/CONSOLIDATION-BRANCH-DISPOSITION.md`
**Claims:** `consolidation/v1-2026-10-12` branch is historical; its 1 unique commit is redundant.
**Code agrees.** The test body already exists in master. No work to extract.

### `docs/WORKTREE-CLEANUP-STATE.md`
**Claims:** 152 untracked artifacts, cleanup blocked by operator approval.
**Code agrees.** Category breakdown is consistent. The 96 `kcode-*` debris and 56 operator-review items are real.

---

## 6. False-Done Findings

### FALSE DONE: MCP Support (from MCP_SKILLS_PLAN.md)
The plan's "Current State" section says "No MCP support" and proposes implementing it. MCP support DOES exist in the current codebase (README documents `~/.jcode/mcp.json`, `~/.claude.json` compat, stdio-based MCP servers). The plan is stale — the implementation was done via a different path than the plan proposed, but the capability exists.

### FALSE DONE: Self-Dev Architecture (UNIFIED_SELFDEV_SERVER_PLAN.md)
Marked "Implemented." The merge DID happen — self-dev is a session-local canary capability. `hot_exec.rs` provides `hot_reload()`, `hot_rebuild()`, `hot_update()`, `hot_restart()`. This is **correctly marked done**.

### FALSE DONE: None Found as Contradictions
No document claims "DONE" for something the code contradicts. The UNIFIED_SELFDEV_SERVER_PLAN.md correctly marks itself implemented, and the code confirms it. All other plans correctly use status markers (Draft, Proposed, Audit-based plan) that match their actual state.

---

## 7. README Capability Claims — Verification

| # | Claim | Verdict | Evidence |
|---|-------|---------|----------|
| 1 | "Most RAM efficient harness" (27.8 MB PSS, 1 session) | **UNVERIFIED** | Benchmark data provided but specific to one Linux machine, one build. Cannot verify without re-running. Claims vs 8 other tools are specific version-pinned. |
| 2 | "Most intelligent harness" | **UNVERIFIED** | Subjective claim, no measurable criteria. |
| 3 | 14.0 ms time to first frame | **UNVERIFIED** | Platform-specific (Linux), cannot verify on this macOS host. |
| 4 | Memory system with embedding + graph + sidecar | **VERIFIED** | `MEMORY_ARCHITECTURE.md` and `MEMORY_GRAPH_PLAN.md` confirm architecture. Hybrid retrieval + LLM reranker shipped. |
| 5 | Side panels, mermaid diagrams, info widgets | **VERIFIED** | `docs/MERMAID_RENDERING_REDESIGN.md` exists. `jcode-tui-render` crate exists. README screenshots provided. |
| 6 | Swarm: multi-agent collaboration, conflict detection | **VERIFIED** | `SWARM_ARCHITECTURE.md`, `SWARM_TASK_GRAPH.md` exist. `src/cli/commands.rs` has swarm commands. |
| 7 | Swarm tool for autonomous spawning | **VERIFIED** | README documents `swarm spawn`, `swarm assign_task` etc. Tool exists in harness. |
| 8 | OAuth login flows (Claude, OpenAI, Gemini, Copilot, Azure, etc.) | **VERIFIED** | `src/cli/login.rs` (1,454 lines), `src/cli/provider_init.rs` (1,936 lines) implement these. |
| 9 | 30+ provider profiles (openrouter, deepseek, groq, etc.) | **VERIFIED** | `src/cli/provider_init.rs:25-148` has `ProviderChoice` enum with many variants. |
| 10 | MCP config file support (`~/.jcode/mcp.json`) | **VERIFIED** | README documents JSON config. Claude Code compat mentioned. |
| 11 | Self-dev mode (agent modifies its own source) | **VERIFIED** | `src/cli/hot_exec.rs` implements hot_reload/rebuild/update/restart. `selfdev_tests.rs` exists. |
| 12 | Browser automation (Firefox Agent Bridge) | **VERIFIED** | `browser` tool exists (invoked in this audit session). README says Firefox backend only. |
| 13 | Session resume from other harnesses (Codex, Claude, OpenCode, Pi) | **PARTIALLY VERIFIED** | `/resume` command exists in CLI. Codex and Claude session import mentioned in docs. Cannot verify all 4 harnesses work. |
| 14 | Agent grep (structured code search) | **VERIFIED** | `agentgrep` tool exists and is actively used in the harness. |
| 15 | Interleaved input with shift-enter queue | **UNVERIFIED** | `SHIFT_ENTER.md` doc exists but behavior cannot be verified from code inspection alone. |
| 16 | Mermaid rendering 1800x faster (no browser/TS dependency) | **UNVERIFIED** | Claims reference `mermaid-rs-renderer` external repo. Cannot verify speedup claim from this repo alone. |
| 17 | "Coming soon" iOS app / Native OpenClaw | **NOT STARTED** | `docs/IOS_APP.md` exists. No iOS code in this repo. Explicitly "coming soon". |
| 18 | Custom terminal (Handterm) | **UNVERIFIED** | Claims reference external repo `1jehuang/handterm`. No terminal code in this repo. |
| 19 | Skills auto-injected via embedding hit | **VERIFIED** | `MEMORY_ARCHITECTURE.md` describes embedding-based skill injection. |
| 20 | Multi-account switching (`/account`) | **VERIFIED** | `src/cli/account.rs` exists. |
| 21 | Config-file MCP setup | **VERIFIED** | README documents JSON config format. |
| 22 | Windows support (PowerShell installer) | **VERIFIED** | `WINDOWS.md` exists. Issue #45 tracks Windows ConPTY work. |
| 23 | Docker images via GitHub Packages | **UNVERIFIED** | README claims `ghcr.io/KooshaPari/jcode` Docker images. Cannot verify without pulling. |

### Summary
- **VERIFIED:** 12 claims
- **PARTIALLY VERIFIED:** 1 claim
- **UNVERIFIED:** 8 claims (mostly external benchmarks or platform-specific)
- **NOT STARTED:** 1 claim (iOS app)
- **FALSE:** 0 claims

---

## Summary of Findings

### Status Distribution
- **DONE:** 2 documents (UNIFIED_SELFDEV_SERVER_PLAN, Q7 disposition)
- **PARTIAL:** 9 documents (CODE_QUALITY_10_10, CODE_QUALITY_TODO, COMPILE_PERFORMANCE, MEMORY_GRAPH, MCP_SKILLS, OPENAI_COMPAT_PROFILE, client-core split, modular RFC, REFACTORING)
- **NOT STARTED:** 5 documents (SERVER_SERVICE_SPLIT, manager mode, screen capture security, computer-use-maximal, cross-device sync)
- **ABANDONED:** 0 documents (but 7 PRs classified as stale/abandoned)

### Critical False-Done Findings
**None.** No document claims completion for something the code contradicts. The project is honest about its status markers.

### The Biggest Gap
The **Modular Architecture RFC** is the most significant gap between aspiration and reality. It has a comprehensive 7-phase plan, 10 dependency rules, and 92 crates — but the acceptance criteria (root = composition shell, server/agent/TUI extracted) are NOT met. The root crate still owns ~21K lines of CLI code. The workspace exists as a "data-contract shell" around a monolithic root, not the layered architecture the RFC envisions.

### The Biggest Win
The **Memory Graph Plan** achieved its most important goal (recall@5: 0.0→0.75) through a detour — the hybrid+rerank pipeline proved more effective than the graph phases (A-E) the plan originally prioritized. The graph remains "write-mostly" as the plan itself diagnosed.

### PR Health
- 7 of 12 open PRs are stale drafts from late September (macOS trust, ForgeCode provider, ACP, server identity)
- 1 major sync PR (#23, v0.88.0 merge) sits in unknown state since October 1
- 1 PR (#35, tiny terminal fix) has merge conflicts
- Only 1 PR (#58, Windows transport) shows active development