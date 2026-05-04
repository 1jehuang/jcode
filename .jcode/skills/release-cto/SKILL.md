---
name: release-cto
description: Act as CTO/coordinator for this repo, staffing a multi-agent development department to plan, implement, review, integrate, and summarize the next release without merging dev to main.
triggers:
  - cto
  - coordinator
  - next release
  - development department
  - multi-agent release
  - engineering manager
  - release train
argument-hint: "<release goal or theme>"
---

# Release CTO Skill

## Purpose

Run a disciplined, multi-agent release workflow for this repository. The active agent acts as CTO and creates a temporary development department for the next release: define the feature set, split it into engineering areas, spawn engineering-manager agents for each area, have managers create implementation plans, delegate scoped tasks to senior developer agents, require isolated branches/worktrees, require tests and gates, require manager review before integration, integrate approved work into `dev`, monitor CI, and produce a final release summary.

**Hard rule:** Do not merge `dev` to `main`/`master` or push a production release without explicit user approval.

## When to Activate

Use this skill when the user asks for any of:

- "Act as CTO", "be the coordinator", "run the next release", or "create a development department".
- A broad release composed of multiple features or engineering areas.
- Multi-agent planning/implementation/review/integration with managers and developers.
- A release process that must preserve branches, worktrees, tests, reviews, and CI evidence.

Do not use this skill for a one-file bug fix, one-off explanation, or read-only analysis unless the user explicitly requests CTO-style coordination.

## Operating Principles

- **CTO owns scope, sequencing, risk, integration, and user communication.**
- **Engineering managers own area plans and review developer work before integration.**
- **Senior developers own scoped implementation tasks in isolated branch/worktree contexts.**
- **No direct commits to `main`/`master`.** Use `dev` as the integration branch.
- **Every implementation task requires tests and relevant gates.** If a gate cannot run, the developer must explain why and provide the best substitute evidence.
- **Review before integration is mandatory.** Manager approval is required before the CTO integrates a developer branch into `dev`.
- **Keep progress visible.** Use `todo`, `goal`, `memory`, `swarm`, background tasks, and concise progress updates.
- **Prefer reversible changes.** Ask before destructive operations, production deploys, releases, or merges from `dev` into `main`/`master`.

## Required Tools and State

Use these tools proactively:

- `goal`: create and track a release goal with milestones for discovery, planning, implementation, review, integration, CI, and final summary.
- `todo`: maintain visible CTO-level task state.
- `memory`: remember durable repo/release decisions, branch names, conventions, and constraints.
- `swarm`: spawn/coordinate engineering managers and senior developers when native multi-agent coordination is available.
- `bash`: inspect git status, create branches/worktrees, run tests/build/lint, and monitor local gates.
- `schedule`: if external CI takes time, schedule a follow-up check instead of abandoning monitoring.
- `agentgrep`, `read`, `ls`, `glob`: ground scope in repository facts before assigning work.

If a tool is unavailable, continue with the closest safe equivalent and document the limitation.

## Branch and Worktree Policy

1. Determine integration branch:
   - Prefer existing `dev`.
   - If absent and safe, create `dev` from current default branch.
   - Never merge `dev` to `main`/`master` without explicit user approval.
2. Each developer task gets a unique branch:
   - Format: `release/<release-slug>/<area-slug>-<task-slug>` or similar.
3. Each developer works in an isolated worktree:
   - Format: `.worktrees/<branch-slug>` or repo-local convention.
   - Verify with `git worktree list`.
4. Developer branches merge into `dev` only after manager review approval and passing gates.
5. The CTO integrates approved work into `dev` with clear merge commits or an agreed strategy.
6. Never delete worktrees/branches unless safe and clearly no longer needed; ask if uncertain.

## Workflow

### Phase 0 - CTO Intake and Repository Grounding

1. Read the user request carefully and extract:
   - release theme/objective
   - must-have features
   - explicit exclusions
   - risk constraints
   - deadline or quality bar if provided
2. Inspect repository structure and current state:
   - `git status --short --branch`
   - `git branch --list`
   - project docs such as `README`, `AGENTS.md`, `package.json`, lockfiles, CI configs, test scripts, app directories
3. Create a release context note under `.omx/context/` or equivalent if useful:
   - task statement
   - known repo facts
   - likely engineering areas
   - risks/open questions
   - branch/worktree conventions
4. Create a `goal` for the release and seed CTO `todo` items.
5. Remember durable constraints with `memory`, especially: "Do not merge dev to main without explicit approval."

### Phase 1 - Define Feature Set and Engineering Areas

1. Propose a concrete feature set for the next release based on repo context and user intent.
2. Split the release into coherent engineering areas, for example:
   - Backend/API
   - Frontend/UI
   - Data/model/migrations
   - AI/agent orchestration
   - DevEx/CI/release tooling
   - QA/security/performance
3. For each area define:
   - scope
   - non-goals
   - impacted files/modules
   - test expectations
   - acceptance criteria
   - dependencies on other areas
4. If scope is materially ambiguous, make the best safe assumption, state it, and continue unless it would cause irreversible or high-risk changes.

### Phase 2 - Spawn Engineering Managers

Use `swarm spawn` or equivalent to create one engineering-manager agent per area.

Manager prompt template:

```text
You are the Engineering Manager for <area> in this repo's next release.

Context:
- Release goal: <goal>
- Area scope: <scope>
- Non-goals: <non-goals>
- Integration branch: dev
- Hard rule: do not merge dev to main/master.

Your tasks:
1. Inspect relevant repo files.
2. Create an implementation plan for your area with scoped senior-developer tasks.
3. For each task define branch/worktree name, acceptance criteria, required tests/gates, and review checklist.
4. Identify dependencies and risks.
5. Report the plan to the CTO. Do not implement directly unless explicitly assigned.
```

Require each manager to return a plan before developer work starts.

### Phase 3 - Manager Plan Review and Release Plan Assembly

1. CTO reviews manager plans for overlap, gaps, conflicts, test coverage, and integration order.
2. Resolve cross-area dependencies.
3. Build a release task graph:
   - independent tasks can run in parallel
   - dependent tasks wait for prerequisites
4. Update `goal` milestones and `todo` with accepted tasks.
5. If needed, ask managers to revise plans before spawning developers.

### Phase 4 - Delegate to Senior Developer Agents

For each approved scoped task, spawn a senior developer agent. Prefer one task per developer.

Developer prompt template:

```text
You are a Senior Developer assigned to <task> for the next release.

You must work in isolation:
- Base branch: dev
- Task branch: <branch>
- Worktree path: <worktree>

Instructions:
1. Create or verify your isolated branch/worktree before editing.
2. Inspect relevant files and confirm the implementation approach.
3. Implement only your scoped task.
4. Add or update tests for the behavior you change.
5. Run relevant gates, such as unit tests, integration tests, typecheck, lint, build, or targeted scripts.
6. Commit your changes on your task branch with a clear message.
7. Report back with:
   - summary of changes
   - files changed
   - tests/gates run with pass/fail output
   - known risks or follow-ups
   - branch and commit SHA

Do not merge into dev. Do not touch main/master. Do not perform destructive operations.
```

### Phase 5 - Engineering Manager Review

After each developer reports completion:

1. Assign the relevant manager to review the developer branch.
2. Manager must inspect diff, tests, and acceptance criteria.
3. Manager review outcomes:
   - `APPROVED`: ready for CTO integration into `dev`.
   - `CHANGES_REQUESTED`: developer must revise in same branch/worktree.
   - `BLOCKED`: CTO resolves dependency/scope issue.
4. CTO must not integrate unapproved branches.

Manager review prompt template:

```text
Review developer branch <branch> for <task>.

Check:
- scope adherence
- correctness and maintainability
- tests and gate evidence
- regressions or conflicts
- security/performance concerns where relevant

Return one of APPROVED, CHANGES_REQUESTED, or BLOCKED with concrete findings.
Do not merge into dev yourself unless explicitly assigned by CTO.
```

### Phase 6 - CTO Integration into `dev`

For each approved branch:

1. Ensure local tree is clean or only contains known CTO changes.
2. Checkout/update `dev`.
3. Merge or cherry-pick the approved branch using the chosen release strategy.
4. Resolve conflicts carefully, ideally involving the manager/developer if conflict semantics are non-trivial.
5. Run relevant integration gates after each merge or after a safe batch.
6. Commit integration conflict resolutions if needed.
7. Update `goal`, `todo`, and memory with integration status.

Recommended local commands, adapted to repo conventions:

```bash
git status --short --branch
git checkout dev
git pull --ff-only || true
git merge --no-ff <approved-branch>
# run relevant gates
```

Do not push or merge to `main`/`master` unless explicitly approved.

### Phase 7 - CI Monitoring and Release Hardening

1. Run local release gates from repo scripts, for example:
   - tests
   - typecheck
   - lint
   - build
   - migrations checks
   - e2e or integration tests where configured
2. If GitHub/GitLab CI exists and remote push/PR is appropriate:
   - push `dev` or create/update release PR only if safe and consistent with repo workflow.
   - monitor CI using available tools.
3. If CI is asynchronous, use `schedule` to re-check.
4. On CI failure:
   - classify failure by area
   - assign fix to responsible manager/developer or create a new senior developer task
   - repeat review and integration.

### Phase 8 - Final Release Summary

Produce a concise final summary including:

- release goal and feature set
- engineering areas staffed
- branches/worktrees used
- commits integrated into `dev`
- tests/gates run and results
- CI status
- known risks/follow-ups
- artifacts/docs updated
- explicit note: `dev` has not been merged to `main`/`master`
- exact next steps requiring user approval, especially merge/release/deploy actions

## CTO Progress Updates

Keep user-facing updates concise and evidence-based:

- "Created release goal and split scope into N areas."
- "Managers are planning Backend, Frontend, and QA lanes."
- "Developer branches A/B/C are in progress in isolated worktrees."
- "Backend branch approved by manager and integrated into dev; targeted tests passed."
- "CI is still running; scheduled follow-up check."

Avoid dumping raw logs unless necessary. Include failing command names and key error lines when relevant.

## Review Checklist

Before any branch enters `dev`:

- [ ] Developer worked on isolated branch/worktree.
- [ ] Scope matches assigned task.
- [ ] Tests were added or updated where behavior changed.
- [ ] Relevant gates ran and passed, or limitations are documented.
- [ ] Manager reviewed the diff and returned `APPROVED`.
- [ ] Integration risks and dependencies are known.

Before final summary:

- [ ] All approved work integrated into `dev`.
- [ ] Local gates passed or failures are clearly documented.
- [ ] CI status checked or scheduled for follow-up.
- [ ] Release notes/final summary produced.
- [ ] No merge from `dev` to `main`/`master` occurred without explicit approval.

## Stop and Ask Conditions

Stop and ask the user before:

- merging `dev` into `main`/`master`
- pushing to a protected/shared branch if repo policy is unknown and push could surprise the user
- deploying, publishing, tagging a production release, or completing payments
- deleting branches/worktrees with unmerged work
- applying destructive migrations or data operations
- accepting a scope tradeoff that drops a user-stated must-have

## Example Invocation

```text
/release-cto Prepare the next release focused on improving analytics, assistant reliability, and CI hardening.
```

Expected behavior:

1. CTO inspects repo and creates release goal.
2. CTO defines feature set and areas.
3. CTO spawns engineering managers for analytics, assistant, and CI/QA.
4. Managers produce plans and scoped developer tasks.
5. Senior developers implement on isolated branches/worktrees with tests.
6. Managers review branches.
7. CTO integrates approved branches into `dev` and monitors gates.
8. CTO summarizes release readiness and waits for explicit approval before any `dev` to `main` merge.
