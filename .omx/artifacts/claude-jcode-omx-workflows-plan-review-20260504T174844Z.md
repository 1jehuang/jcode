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

Goal: Add OMX-style workflow modes to Jcode:
- /ulw / /ultrawork: native parallel execution protocol
- /ralph: persistent completion loop with verification gates
- /ralplan: Planner → Architect → Critic consensus planning
- Optional CLI entrypoints later, but first implement prompt/skill-native behavior

Key principle: use Jcode’s native swarm/session/background runtime, not tmux.

Phase 1: Workflow mode state
Add durable workflow state under .jcode/state/workflows/{ralph,ralplan,ultrawork}.json. State includes mode, active, phase, iteration, max_iterations, task, session_id, timestamps, verification, agents.
Acceptance: start/update/complete/cancel workflow state, survives restart/compaction, UI/swarm widgets can read active mode.
Likely files: src/skill.rs, src/tool/skill.rs, src/server/swarm*.rs, new src/workflow_mode.rs.

Phase 2: Keyword and skill activation
Support /ralph, /ralplan, /ulw, /ultrawork, ralph, ulw, don't stop, must complete, consensus plan. Activation detects workflow, seeds state, loads skill instructions, injects mode reminder into active turn. Prompt-side /ralph does not spawn new session automatically. /ulw spawns only when independent lanes are identified. /ralplan plans, not edits.

Phase 3: Ultrawork native implementation
Ground task, define acceptance criteria, split independent lanes, use native Jcode swarm/subagent only where parallelism helps, keep shared-file edits local, run lightweight verification.

Phase 4: Ralplan consensus planning
Planner draft -> Architect review -> Critic review -> revise until approval/max iterations. Architect and Critic sequential. Output .jcode/plans/<slug>.md/json with scope, non-goals, implementation steps, risks, acceptance criteria, test plan, recommended execution mode, suggested lanes.

Phase 5: Ralph persistent completion loop
Seed state, track TODOs, execute until done, spawn helpers when useful, run required verification, run reviewer/architect check, prevent premature completion if gates fail. Ralph can finish only when all TODOs complete, verification commands pass, changed files reviewed, no active spawned agents remain, final evidence recorded.

Phase 6: Stop/completion gates
Native stop checks in Jcode's turn loop. If Ralph active and incomplete, continue/remind. Ultrawork has weaker gate: require lightweight evidence. Ralplan requires plan artifact and critic verdict.

Phase 7: UI/status integration
Show active workflow in TUI/statusline and link spawned agents to workflow.

Phase 8: Optional CLI entrypoints
jcode ralph/ralplan/ultrawork start new sessions with state pre-seeded.

Recommended order: state module, skill docs, activation plumbing, ultrawork, ralplan, ralph, UI, CLI.
MVP: state model, /ulw, /ralplan, /ralph, prompt-side activation, status display, no hard stop hooks. Second PR: Ralph gates, spawned-agent tracking, plan artifacts, reviewer verification loop.

Review request:
1. Identify architectural risks, hidden complexity, and likely integration points missing from the plan.
2. Recommend changes to phase order and MVP boundaries.
3. Propose specific acceptance tests, unit/integration/e2e.
4. Point out where the plan may conflict with Jcode-native behavior or user expectations.
5. Return a severity-ranked review: blockers, high priority, medium priority, nice-to-have.
```

## Claude output (raw)

