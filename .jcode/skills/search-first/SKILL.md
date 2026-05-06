---
name: search-first
description: Research-before-coding workflow. Use this skill whenever a task may already be solved by existing repository code, an existing skill, a built-in jcode tool, MCP integration, or a maintained dependency, even if the user jumps straight to implementation.
allowed-tools: read, grep, agentgrep, codesearch, ls, bash
---

# Search First

Use this workflow before adding new code, dependencies, abstractions, or project structure.

## When To Use

- Starting a feature with likely existing patterns.
- Adding a dependency, provider, MCP server, adapter, wrapper, or tool.
- Creating a helper, utility, command, prompt, or skill.
- Fixing behavior where similar behavior may already exist elsewhere.
- Choosing between direct code changes and a built-in jcode capability.

## Decision Order

1. Existing repository code or pattern.
2. Existing project-local skill in `.jcode/skills/`.
3. Existing jcode tool or runtime feature.
4. Existing MCP server or configured integration.
5. Maintained external dependency or reference implementation.
6. Net-new custom code.

## Repository-First Workflow

1. Identify manifests and project shape: `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, or equivalents.
2. Identify entrypoints, command handlers, services, CLIs, tools, tests, and docs related to the request.
3. Search for similar nouns, verbs, protocol messages, error strings, config names, and test fixtures.
4. Read the smallest set of files needed to understand the local pattern.
5. Implement the smallest viable change that matches that pattern.

Useful starting commands:

```bash
rg -n "keyword|type|command|error|config" .
rg --files
```

Prefer `agentgrep` or `codesearch` when semantic or structural context would reduce the amount of file reading.

## Adopt Vs Build

Adopt or wrap existing behavior when:

- The capability already exists in repo code.
- A jcode tool, command, provider, skill, or MCP integration already provides the behavior.
- A small maintained dependency replaces a larger custom implementation without broad coupling.

Build custom code when:

- No existing tool or package matches the requirement.
- Project constraints require tighter control.
- The wrapper would be smaller and easier to maintain than introducing a broad dependency.

## Anti-Patterns

- Creating utilities without searching the repo first.
- Adding a dependency before checking existing code and configured tools.
- Building a new workflow when a skill, prompt, or small config change is enough.
- Inventing file paths, commands, APIs, or runtime behavior before verifying they exist.

## Output Expectations

When closing the task, include:

- Repository context found, with real file paths.
- Existing patterns reused or intentionally not reused.
- Files changed.
- Tests or checks run.
- Gaps, risks, and follow-up work.
