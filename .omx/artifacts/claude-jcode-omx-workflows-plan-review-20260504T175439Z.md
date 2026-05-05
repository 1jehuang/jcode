# Claude Review: Jcode OMX-style workflow plan

## Original user task
Can you have claude review the plan

## Final prompt sent to Claude CLI
```text
You are reviewing an implementation plan for adding OMX-style workflow modes to Jcode, an open-source Rust agent harness. Please perform a rigorous architecture review.

Context:
- Jcode already has native skills, subagents, swarm/session orchestration, background tasks, and TUI widgets.
- OMX has prompt-side workflows: ultrawork (parallel execution protocol), ralph (persistent completion loop), ralplan (Planner -> Architect -> Critic consensus planning), plus omx team as tmux worker runtime.
- The proposed Jcode design intentionally uses native Jcode swarm/session runtime instead of tmux.

Plan to review:
Goal: Add OMX-style workflow modes to Jcode: /ulw / /ultrawork, /ralph, /ralplan. Use Jcode native swarm/session/background runtime, not tmux.

Phase 1: Workflow mode state under .jcode/state/workflows/{ralph,ralplan,ultrawork}.json with mode, active, phase, iteration, task, session_id, verification, agents.
Phase 2: Keyword and skill activation for /ralph, /ralplan, /ulw, ralph, ulw, don't stop, must complete, consensus plan. Activation seeds state, loads skill instructions, injects reminder. Prompt-side /ralph does not spawn new session automatically.
Phase 3: Ultrawork grounds task, defines acceptance criteria, splits independent lanes, uses native swarm/subagent only where helpful, keeps shared-file edits local, lightweight verification.
Phase 4: Ralplan: Planner draft -> Architect review -> Critic review -> revise until approval/max iterations. Architect and Critic sequential. Output .jcode/plans/<slug>.md/json with scope, non-goals, steps, risks, acceptance criteria, test plan, recommended execution mode, suggested lanes.
Phase 5: Ralph: persistent completion loop, TODO tracking, execution, helper agents, verification, reviewer/architect check, prevent premature completion. Finish only when TODOs complete, verification passes, changed files reviewed, no active spawned agents, final evidence recorded.
Phase 6: Stop/completion gates in Jcode turn loop.
Phase 7: UI/status integration.
Phase 8: Optional CLI entrypoints.
MVP: state model, /ulw, /ralplan, /ralph, prompt-side activation, status display, no hard stop hooks. Second PR: Ralph gates, spawned-agent tracking, plan artifacts, reviewer verification loop.

Review request:
1. Identify architectural risks, hidden complexity, and missing integration points.
2. Recommend phase order and MVP boundary changes.
3. Propose unit/integration/e2e acceptance tests.
4. Point out conflicts with Jcode-native behavior or user expectations.
5. Return severity-ranked review: blockers, high priority, medium priority, nice-to-have.
```

## Claude output (raw)

The prior review artifacts at `.omx/artifacts/` are empty stubs (37 and 63 lines, just the prompt header), so this is a clean review. Here is my critique grounded in the actual jcode workspace.

---

# Architecture Review: OMX-style Workflow Modes for Jcode

## Summary
The plan ports OMX's prompt-side workflows but treats jcode as if it were Claude Code — a generic harness with a filesystem and a CLI. It is not. Jcode is a 35-crate Rust workspace with strongly typed primitives that already cover most of what the plan re-implements ad hoc:

- **`jcode-plan::PlanItem`** already has `id`, `status`, `blocked_by`, `assigned_to`, `file_scope`, `subsystem`, plus `summarize_plan_graph` (ready/blocked/active/cycle), `next_runnable_item_ids`, and `newly_ready_item_ids`. This *is* a Ralph executor's task graph.
- **`jcode-task-types::Goal`** has `milestones`, `success_criteria`, `status` (Active/Blocked/Completed), `blockers`, `current_milestone_id`, `progress_percent`. This *is* a Ralph completion model.
- **`jcode-agent-runtime`** exposes `SoftInterruptQueue`, `InterruptSignal`, `GracefulShutdownSignal`, `BackgroundToolSignal`. These *are* the stop/completion gates for Phase 6.
- **`jcode-background-types::BackgroundTaskStatus`** with `{Running, Completed, Superseded, Failed}` *is* the spawned-agent tracking signal for "no active agents" gate.

The plan never names these. That is the central risk: it invents a parallel state system in `.jcode/state/workflows/*.json` that drifts from the typed runtime instead of plugging into it.

---

## Severity-ranked findings

### 🔴 Blockers (must resolve before MVP)

**B1. Plan does not specify which crate(s) own this code.**
35 crates, no `jcode-workflow` or `jcode-skill` crate visible, and `jcode-core` has no skill-related symbols (grep confirms). Without this, Phase 1 reviewers cannot say yes/no to anything. **Action:** decide between (a) new `jcode-workflow` crate that depends on `jcode-plan` + `jcode-task-types` + `jcode-agent-runtime`, or (b) a `workflow` module inside `jcode-agent-runtime`. State this explicitly.

**B2. Reinventing the plan graph.**
Phase 4 outputs `.jcode/plans/<slug>.md/json` with "scope, non-goals, steps, risks, acceptance criteria, test plan, recommended execution mode, suggested lanes." Phase 5 has Ralph track TODOs separately. But `jcode-plan::PlanItem` + `summarize_plan_graph` already model exactly this, with cycle detection and ready/blocked queues already tested. **Action:** ralplan output should serialize as `Vec<PlanItem>` (with extra `acceptance_criteria` / `risks` fields added if needed), and Ralph's TODO tracker should be the same struct — not a parallel "todos" list.

**B3. Stop/completion gates without naming the existing primitives.**
Phase 6 says "stop/completion gates in Jcode turn loop." But the turn loop already integrates `InterruptSignal`, `SoftInterruptQueue`, `GracefulShutdownSignal`. Ralph's "do not stop until verification passes" must be implemented as a *suppression rule* on these signals (e.g., consume `GracefulShutdownSignal` only if `Ralph::is_complete()` returns true), not a new gate. Otherwise Ctrl-C behavior diverges from user expectation and silently breaks. **Action:** spec the exact signal interaction in Phase 6 *before* shipping the prompt-side activation in MVP, because activation without a defined stop semantics ships a footgun.

**B4. "No active spawned agents" gate is undefined.**
Ralph's completion criterion includes "no active spawned agents." Jcode tracks `BackgroundTaskStatus` per task and has session/swarm runtime — but the plan doesn't say *which* enumeration counts. Subagent? Background task? Swarm session child? Each has different lifecycle. **Action:** define "active" as a query against a specific source of truth (likely `BackgroundTaskStatus::Running` filtered by session_id), and write that query first.

---

### 🟠 High priority

**H1. State files in `.jcode/state/workflows/*.json` will desync from the session.**
Three separate JSON files for ralph/ralplan/ultrawork, each with `session_id`, `phase`, `iteration`, `agents`. There is no transactional model with the session store, no schema versioning, no migration story, and crash recovery semantics aren't specified. If a session crashes mid-iteration, what happens on next launch? **Action:** either (a) put workflow state inside the session store (as a `WorkflowState` field on session record) or (b) use atomic-write + schema_version + a documented "stale state" detection rule.

**H2. Mode mutual exclusion / composition is undefined.**
Can `/ralph` and `/ulw` be active at once? Can `/ralph` invoke ralplan internally for re-planning? OMX allows this composition; the plan has three independent state files implying yes, but with no precedence rules. **Action:** define a single `WorkflowMode` enum with explicit composition rules, or document "at most one active at a time."

**H3. Skill activation pathway is hand-waved.**
"Activation seeds state, loads skill instructions, injects reminder." Where does the injection happen — system prompt prefix, tool-result reminder, separate channel? Jcode's skills (`.jcode/skills/<name>/SKILL.md`) currently load via what mechanism? Phase 2 needs to point at the existing skill loader and say "we add a `WorkflowSkill` variant" or similar. Without that, the MVP can't be reviewed.

**H4. Ralplan's Architect→Critic loop has no termination guarantee.**
"Revise until approval/max iterations." If Critic never approves, do we ship the last draft or fail? What does the user see during the loop — silent agents or streamed deltas? In a TUI this matters. **Action:** define max_iterations default (suggest 3), explicit "approved with caveats" exit, and per-iteration TUI status update.

**H5. Keyword activation list is dangerous.**
"don't stop", "must complete", "consensus plan" as activation triggers will fire on innocent prose ("I don't stop the build before tests" → ralph mode). **Action:** require either a leading slash command OR a high-confidence keyword (e.g., bare "ralph" alone, or quoted phrase). At minimum, log when keyword activation fires so users can see why their mode flipped.

**H6. Verification pipeline isn't specified.**
"Verification passes" is the gate for Ralph completion, but the plan doesn't say what verifies. `cargo test`? Project-detected? User-supplied? `jcode-config-types` may have a verify hook — this needs to be wired explicitly. Without it, Ralph either over-runs or never completes.

---

### 🟡 Medium priority

**M1. MVP boundary is wrong.** Shipping `/ralph` in MVP without "hard stop hooks" means MVP Ralph can be Ctrl-C'd out trivially, which makes it effectively a no-op vs. just running the underlying work without a mode. Ship `/ralplan` + `/ulw` in MVP (both are bounded operations that complete naturally) and defer all of `/ralph` to PR 2 with the gates. The current MVP/PR2 split puts the dangerous part first and the safety rail second.

**M2. Suggested phase reorder:**
1. Workflow crate skeleton + `WorkflowState` integrated into session store (replaces Phase 1).
2. Skill activation API extension + keyword guard rules (Phase 2, but with H5 fix).
3. `/ralplan` end-to-end emitting `Vec<PlanItem>` into `jcode-plan` (Phase 4, ahead of others; produces test fixtures everyone else needs).
4. `/ulw` consuming a plan (Phase 3).
5. UI/status (Phase 7, before Ralph so Ralph is observable).
6. Ralph: gates first, loop second (Phases 5+6 inverted).
7. CLI entrypoints (Phase 8, last).

**M3. "Lightweight verification" in ultrawork is ambiguous.** Define it: "after each lane, run `cargo check -p <crate>` if file_scope is bounded to one crate, else skip." Otherwise different reviewers will assume different things.

**M4. TUI integration risk.** Phase 7 is "optional" in the plan but a workflow that runs many iterations without visible state is a UX disaster. Promote to non-optional and define the widget contract before Phase 5.

**M5. No persona / model routing story.** OMX uses different personas/models per role (Planner/Architect/Critic). Jcode has multiple providers — the plan should say whether ralplan roles route to different models or all use the active one. This affects cost and quality.

**M6. Ralplan output dual format (`.md/json`) doubles maintenance.** Pick one as canonical (json, since it round-trips through `PlanItem`) and render markdown on demand.

---

### 🟢 Nice-to-have

- **N1.** Add a `--dry-run` flag to all three modes that prints the activation effect without mutating state — useful for tests and for the keyword-collision concern (H5).
- **N2.** Telemetry: emit a `workflow.activated`, `workflow.iteration`, `workflow.completed` event series so the existing `0005_workflow_turn_telemetry.sql` migration can be reused.
- **N3.** A `/workflow status` slash command for users to see which modes are active and inspect state, instead of grepping JSON.
- **N4.** Document the failure mode where a Ralph session is force-killed and the JSON state says "active" forever — provide a `jcode workflow reset` escape hatch.

---

## Acceptance tests

### Unit
- `WorkflowState` round-trips through serde for each variant; unknown fields preserved.
- Keyword activator: positive set fires on `/ralph`, `ralph`, `must complete`; negative set does not fire on "I won't stop here", "the must-complete list".
- Ralplan termination: convergence within `max_iterations`, fall-through behavior, "approved with caveats" path.
- Ralph completion gate: returns false if any of {todos open, verification not run, verification failed, active background tasks > 0, unreviewed changed files}.
- Stop signal interaction: `GracefulShutdownSignal` raised during Ralph → Ralph emits "completion blocked: <reason>" then yields if user re-signals within N seconds (escape hatch).

### Integration
- `/ralplan "add caching to API"` produces `Vec<PlanItem>` that passes `summarize_plan_graph` with no cycles and at least one ready item.
- `/ulw <plan>` executes lanes respecting `blocked_by`, never runs two items with overlapping `file_scope` concurrently.
- Activation seeds state file atomically (kill mid-write → next launch detects partial write and recovers).
- Mode composition: starting `/ralph` while `/ulw` active either rejects or composes per the rule decided in H2.
- Ralph + real cargo failure: Ralph re-iterates and eventually gives up at iteration cap with clear final status.

### E2E
- TUI snapshot test: status widget shows mode, phase, iteration, current_milestone, ready/blocked counts, age.
- A scripted run of `/ralplan → /ulw → ralph` on a synthetic repo completes in bounded time and the resulting commits match the plan's acceptance criteria.
- Crash-resume: kill the process during ralplan iteration 2, restart, verify state recovers or is flagged stale.

---

## Conflicts with jcode-native behavior

1. **Stop semantics.** Users on `Ctrl-C` expect to interrupt. Ralph's "don't stop" silently overrides this. Must be visible in TUI and overridable with double-Ctrl-C.
2. **Plan ownership.** `jcode-plan` already is the swarm plan. Adding `.jcode/plans/<slug>.json` as a parallel concept will confuse anyone touching swarm code.
3. **Goal ownership.** `jcode-task-types::Goal` is the persistent goal. Ralph's "task" should be a `Goal` reference, not a free-text string in a JSON file — otherwise the "completion" notion drifts from the rest of the harness.
4. **Skill loading.** `.jcode/skills/<name>/SKILL.md` is the existing convention. Workflows should be skills, not a parallel "workflow" concept, unless there's a specific reason (state machine? gates?) — and that reason should be in the plan.

---

## Bottom line
The plan's intent (use native runtime, avoid tmux) is right. The execution conflates "OMX-style prompts" with "OMX-style state model" and bypasses jcode's typed primitives. **Before MVP merges**, the team should rewrite Phase 1 + Phase 4 to consume `jcode-plan` and `jcode-task-types` directly, name the owning crate, and pin down the stop-signal interaction. Then ship `/ralplan` and `/ulw` first, defer `/ralph` to a follow-up PR where the gates are the centerpiece.

## Concise summary
The prior review artifacts at `.omx/artifacts/` are empty stubs (37 and 63 lines, just the prompt header), so this is a clean review. Here is my critique grounded in the actual jcode workspace.

---

# Architecture Review: OMX-style Workflow Modes for Jcode

## Summary
The plan ports OMX's prompt-side workflows but treats jcode as if it were Claude Code — a generic harness with a filesystem and a CLI. It is not. Jcode is a 35-crate Rust workspace with strongly typed primitives that already cover most of what the plan re-implements ad hoc:

- **`jcode-plan::PlanItem`** already has `id`, `status`, `blocked_by`, `assigned_to`, `file_scope`, `subsystem`, plus `summarize_plan_graph` (ready/blocked/active/cycle), `next_runnable_item_ids`, and `newly_ready_item_ids`. This *is* a Ralph executor's task graph.
- **`jcode-task-types::Goal`** has `milestones`, `success_criteria`, `status` (Active/Blocked/Completed), `blockers`, `current_milestone_id`, `progress_percent`. This *is* a Ralph completion model.
- **`jcode-agent-runtime`** exposes `SoftInterruptQueue`, `InterruptSignal`, `GracefulShutdownSignal`, `BackgroundToolSignal`. These *are* the stop/completion gates for Phase 6.
- **`jcode-background-types::BackgroundTaskStatus`** with `{Running, Completed, Superseded, Failed}` *is* the spawned-agent tracking signal for "no active agents" gate.

The plan never names these. That is the central risk: it invents a parallel state system in `.jcode/state/workflows/*.json` that drifts from the typed runtime instead of plugging into it.

---

## Severity-ranked findings

### 🔴 Blockers (must resolve before MVP)

**B1. Plan does not specify which crate(s) own this code.**
35 crates, no `jcode-workflow` or `jcode-skill` crate visible, and `jcode-core` has no skill-related symbols (grep confirms). Without this, Phase 1 reviewers cannot say yes/no to anything. **Action:** decide between (a) new `jcode-workflow` crate that depends on `jcode-plan` + `jcode-task-types` + `jcode-agent-runtime`, or (b) a `workflow` module inside `jcode-agent-runtime`. State this explicitly.

**B2. Reinventing the plan graph.**
Phase 4 outputs `.jcode/plans/<slug>.md/json` with "scope, non-goals, steps, risks, acceptance criteria, test plan, recommended execution mode, suggested lanes." Phase 5 has Ralph track TODOs separately. But `jcode-plan::PlanItem` + `summarize_plan_graph` already model exactly this, with cycle detection and ready/blocked queues already tested. **Action:** ralplan output should serialize as `Vec<PlanItem>` (with extra `acceptance_criteria` / `risks` fields added if needed), and Ralph's TODO tracker should be the same struct — not a parallel "todos" list.

**B3. Stop/completion gates without naming the existing primitives.**
Phase 6 says "stop/completion gates in Jcode turn loop." But the turn loop already integrates `InterruptSignal`, `SoftInterruptQueue`, `GracefulShutdownSignal`. Ralph's "do not stop until verification passes" must be implemented as a *suppression rule* on these signals (e.g., consume `GracefulShutdownSignal` only if `Ralph::is_complete()` returns true), not a new gate. Otherwise Ctrl-C behavior diverges from user expectation and silently breaks. **Action:** spec the exact signal interaction in Phase 6 *before* shipping the prompt-side activation in MVP, because activation without a defined stop semantics ships a footgun.

**B4. "No active spawned agents" gate is undefined.**
Ralph's completion criterion includes "no active spawned agents." Jcode tracks `BackgroundTaskStatus` per task and has session/swarm runtime — but the plan doesn't say *which* enumeration counts. Subagent? Background task? Swarm session child? Each has different lifecycle. **Action:** define "active" as a query against a specific source of truth (likely `BackgroundTaskStatus::Running` filtered by session_id), and write that query first.

---

### 🟠 High priority

**H1. State files in `.jcode/state/workflows/*.json` will desync from the session.**
Three separate JSON files for ralph/ralplan/ultrawork, each with `session_id`, `phase`, `iteration`, `agents`. There is no transactional model with the session store, no schema versioning, no migration story, and crash recovery semantics aren't specified. If a session crashes mid-iteration, what happens on next launch? **Action:** either (a) put workflow state inside the session store (as a `WorkflowState` field on session record) or (b) use atomic-write + schema_version + a documented "stale state" detection rule.

**H2. Mode mutual exclusion / composition is undefined.**
Can `/ralph` and `/ulw` be active at once? Can `/ralph` invoke ralplan internally for re-planning? OMX allows this composition; the plan has three independent state files implying yes, but with no precedence rules. **Action:** define a single `WorkflowMode` enum with explicit composition rules, or document "at most one active at a time."

**H3. Skill activation pathway is hand-waved.**
"Activation seeds state, loads skill instructions, injects reminder." Where does the injection happen — system prompt prefix, tool-result reminder, separate channel? Jcode's skills (`.jcode/skills/<name>/SKILL.md`) currently load via what mechanism? Phase 2 needs to point at the existing skill loader and say "we add a `WorkflowSkill` variant" or similar. Without that, the MVP can't be reviewed.

**H4. Ralplan's Architect→Critic loop has no termination guarantee.**
"Revise until approval/max iterations." If Critic never approves, do we ship the last draft or fail? What does the user see during the loop — silent agents or streamed deltas? In a TUI this matters. **Action:** define max_iterations default (suggest 3), explicit "approved with caveats" exit, and per-iteration TUI status update.

**H5. Keyword activation list is dangerous.**
"don't stop", "must complete", "consensus plan" as activation triggers will fire on innocent prose ("I don't stop the build before tests" → ralph mode). **Action:** require either a leading slash command OR a high-confidence keyword (e.g., bare "ralph" alone, or quoted phrase). At minimum, log when keyword activation fires so users can see why their mode flipped.

**H6. Verification pipeline isn't specified.**
"Verification passes" is the gate for Ralph completion, but the plan doesn't say what verifies. `cargo test`? Project-detected? User-supplied? `jcode-config-types` may have a verify hook — this needs to be wired explicitly. Without it, Ralph either over-runs or never completes.

---

### 🟡 Medium priority

**M1. MVP boundary is wrong.** Shipping `/ralph` in MVP without "hard stop hooks" means MVP Ralph can be Ctrl-C'd out trivially, which makes it effectively a no-op vs. just running the underlying work without a mode. Ship `/ralplan` + `/ulw` in MVP (both are bounded operations that complete naturally) and defer all of `/ralph` to PR 2 with the gates. The current MVP/PR2 split puts the dangerous part first and the safety rail second.

**M2. Suggested phase reorder:**
1. Workflow crate skeleton + `WorkflowState` integrated into session store (replaces Phase 1).
2. Skill activation API extension + keyword guard rules (Phase 2, but with H5 fix).
3. `/ralplan` end-to-end emitting `Vec<PlanItem>` into `jcode-plan` (Phase 4, ahead of others; produces test fixtures everyone else needs).
4. `/ulw` consuming a plan (Phase 3).
5. UI/status (Phase 7, before Ralph so Ralph is observable).
6. Ralph: gates first, loop second (Phases 5+6 inverted).
7. CLI entrypoints (Phase 8, last).

**M3. "Lightweight verification" in ultrawork is ambiguous.** Define it: "after each lane, run `cargo check -p <crate>` if file_scope is bounded to one crate, else skip." Otherwise different reviewers will assume different things.

**M4. TUI integration risk.** Phase 7 is "optional" in the plan but a workflow that runs many iterations without visible state is a UX disaster. Promote to non-optional and define the widget contract before Phase 5.

**M5. No persona / model routing story.** OMX uses different personas/models per role (Planner/Architect/Critic). Jcode has multiple providers — the plan should say whether ralplan roles route to different models or all use the active one. This affects cost and quality.

**M6. Ralplan output dual format (`.md/json`) doubles maintenance.** Pick one as canonical (json, since it round-trips through `PlanItem`) and render markdown on demand.

---
