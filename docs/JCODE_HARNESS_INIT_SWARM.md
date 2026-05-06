# jcode-harness Init Swarm

`/init` is the interactive project bootstrap command for the standalone `jcode-harness` product direction. It is intentionally different from a static file generator: the static scaffolding is only phase 0. By default, `/init` immediately queues an LLM-driven multi-agent swarm analysis turn.

## Command forms

```text
/init
/init --force
/init --yes
/init --force --yes
/init --no-swarm
```

- `--force`: overwrite existing generated scaffold files.
- `--yes`: use non-interactive defaults and leave unanswered questions in `.jcode/INIT_QUESTIONS.md`.
- `--no-swarm`: write deterministic scaffold only and do not queue the LLM/swarm turn. This is mainly for tests, offline recovery, or users who want to review files before analysis.

## Phase model

`/init` now has two layers:

1. **Deterministic scaffold phase**
   - Runs synchronously inside the command handler.
   - Creates/updates `AGENTS.md`, `.jcode/INIT_REPORT.md`, `.jcode/INIT_QUESTIONS.md`, `.jcode/SKILLS_PLAN.md`, `.jcode/MCP_PLAN.md`, `.jcode/mcp.json`, side-panel files, memory wiki layout, and `.jcode/init/*` swarm analysis files.

2. **LLM/swarm analysis phase**
   - Queued as a hidden system reminder so the active model performs real analysis after the static scaffold exists.
   - Uses multiple swarm agents in parallel.
   - Blocks synthesis until discovery agents have reported or are explicitly marked blocked.

## Required swarm roles

The queued LLM turn is instructed to spawn or start parallel agents for at least:

- `architect`: repository structure, architecture boundaries, core workflows, high-risk areas.
- `qa`: test commands, CI gaps, validation strategy, risky untested behavior.
- `documenter`: README/docs/onboarding/AGENTS.md improvements and missing project context.
- `tooling-security`: package managers, MCP candidates, secrets boundaries, automation risks.

## Blocking dependencies

The LLM prompt enforces this order:

1. Create todos for discovery fan-out, barrier wait, synthesis, verification plan, final status.
2. Spawn/start swarm agents in parallel.
3. Await all discovery reports, or record blocked agents explicitly.
4. Synthesize only after the barrier.
5. Update `.jcode/init/SWARM_ANALYSIS_REPORT.md`, relevant plans, and side panel.
6. Complete or mark todos blocked with reasons.

This makes dependent synthesis blocking while still using multiple agents for independent discovery.

## Generated files

Additional `/init` files:

- `.jcode/init/SWARM_ANALYSIS_PLAN.md`: deterministic plan given to the LLM/swarm turn.
- `.jcode/init/SWARM_ANALYSIS_REPORT.md`: stub written by static init, intended to be replaced by the LLM synthesis.

## Safety rules

The queued LLM turn is instructed to:

- never store secrets, tokens, private keys, `.env` values, or credentials;
- avoid inventing validation commands not supported by repository evidence;
- mark unknowns and human questions honestly;
- update side-panel status with evidence-backed findings only.

## Test coverage

Current tests verify that:

- static project init creates the swarm analysis plan/report files;
- `/init` queues the hidden swarm analysis prompt by default;
- the prompt includes required roles and blocking barrier language;
- `/init --no-swarm` writes scaffold without queueing the LLM turn;
- invalid flags show the updated usage.
