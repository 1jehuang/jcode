---
name: promptify
description: Convert vague or raw engineering requests into structured, anti-hallucination implementation prompts. Use this skill whenever the user asks to improve, rewrite, clarify, harden, or prepare a task prompt for an agent, even if they only paste a rough request.
allowed-tools: read, grep, agentgrep, codesearch, ls, bash
---

# Promptify

Turn raw user input into a technical prompt that is specific, testable, and suitable for engineering agents.

## Goal

Generate a final prompt that:

- Reduces ambiguity.
- Prevents invented files, APIs, endpoints, tables, libraries, or behavior.
- Requires repository evidence before implementation.
- Requires verifiable evidence after implementation: diff, tests, and results.
- Includes acceptance criteria that can be checked by tests, commands, diff review, or inspection.

## Step 1: Completeness Check

Evaluate the raw input against these criteria.

| Criterion | Rule |
|---|---|
| Word count | Fewer than 60 words means low information. |
| Action verb | Missing a clear verb such as implement, fix, add, create, migrate, refactor, optimize, or integrate means low information. |
| Technical domain | Missing a domain such as backend, frontend, database, auth, infra, API, DevOps, Kubernetes, CLI, TUI, provider, memory, browser, safety, or protocol means low information. |
| Constraints | Missing a constraint, acceptance criterion, validation requirement, or output format means low information. |

Set `needs_research=true` if any criterion fails. Otherwise set `needs_research=false`.

Always begin the response with:

```text
Completeness evaluation: needs_research=true|false
Reason: <brief reason>
```

## Step 2: Research Before Prompting When Needed

When `needs_research=true`, inspect local context before generating the final prompt.

Required local research:

- Map manifests: `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, or equivalents.
- Identify entrypoints, CLIs, servers, tools, providers, protocols, or UI surfaces.
- Identify test locations and patterns.
- Search for similar implementations.
- Cite real files and modules. Do not invent paths.

External research is optional and only allowed when tools and network access are available. If unavailable, say so in the assumptions instead of inventing references.

## Step 3: Generate The Structured Prompt

After the completeness evaluation, include exactly these sections.

## 1) Final Prompt

```xml
<objective>
Clear objective derived from the user input and any repository research.
</objective>

<anti_hallucination_constraints>
- Do not invent files, commands, endpoints, tables, providers, tools, libraries, or behavior.
- Map and cite real repository files before coding.
- If a critical requirement is missing, stop and list "Requirement Blockers".
- Require verifiable evidence: diff by file, tests/checks run, and results.
- Do not claim completion without acceptance-check evidence.
</anti_hallucination_constraints>

<minimum_functional_scope>
Smallest useful scope derived from the request and research.
</minimum_functional_scope>

<required_deliverables>
Concrete deliverables such as code, tests, docs, config, migrations, prompts, skills, or scripts.
</required_deliverables>

<required_response_format>
The agent must respond with:
1. Repository context found, with real files.
2. Implementation plan with small steps.
3. Changes applied, file by file.
4. Tests/checks executed and results.
5. Gaps, risks, and next steps.
</required_response_format>

<acceptance_criteria>
Objective criteria that can be validated by test, command, diff, or inspection.
</acceptance_criteria>
```

## 2) Assumptions

List all assumptions used to fill gaps. Separate what was stated by the user from what was inferred from repository context.

## 3) Open Questions

List only questions that block safe execution. Do not list nice-to-haves.

## Quality Checklist

Before answering, verify:

- The completeness evaluation appears at the top.
- The prompt is specific to the request.
- Anti-hallucination constraints are present.
- Acceptance criteria are testable.
- The required response format is present.
- If `needs_research=true`, repository context and assumptions are present.
- Open questions are limited to execution blockers.
