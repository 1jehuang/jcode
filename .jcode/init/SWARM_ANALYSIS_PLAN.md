# Swarm Init Analysis Plan

Generated: 2026-05-06T21:04:44.579090031+00:00

This file is the deterministic plan that `/init` gives to the LLM-driven swarm analysis turn. The static init pass creates scaffolding first; the queued LLM turn must then run parallel agents and synthesize their findings.

## Detected stack seed

- Rust

## Blocking phase order

1. **Discovery fan-out**: spawn parallel agents for architecture, testing/quality, documentation/onboarding, and tooling/MCP/security.
2. **Barrier**: wait for all discovery agents to report before writing final recommendations.
3. **Synthesis**: merge findings into `.jcode/init/SWARM_ANALYSIS_REPORT.md`, `.jcode/SKILLS_PLAN.md`, `.jcode/MCP_PLAN.md`, and side-panel status pages.
4. **Verification plan**: record commands that should be run before real work begins.

## Required swarm roles

- `architect`: map project structure, boundaries, risks, and core workflows.
- `qa`: identify tests, validation commands, CI gaps, and high-risk untested areas.
- `documenter`: inspect README/docs/onboarding gaps and propose project-specific AGENTS.md improvements.
- `tooling-security`: inspect package managers, MCP candidates, secrets boundaries, and automation risks.

## Output requirements

- Keep generated content project-specific.
- Do not invent commands that are not supported by repository files.
- Do not store secrets or environment values.
- Clearly mark unknowns and questions for humans.
