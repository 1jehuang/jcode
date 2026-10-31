# Agent definitions

An agent definition is a Markdown file with YAML frontmatter, compatible with
Claude Code's `.claude/agents/*.md`. The body is the persona. The swarm tool's
`spawn` action takes an optional `agent` name, and the new worker receives the
persona as a hidden message at the start of its transcript. The system prompt is
never replaced, so prompt caching stays stable.

```markdown
---
name: aidlc-architect-agent      # required: a-z, 0-9, '-', '_' (max 256 chars)
description: Solutions architect # required
---
You are a senior solutions architect ...
```

```text
swarm spawn label="design" agent="aidlc-architect-agent" prompt="Design the API"
```

## Where definitions live

First match by `name` wins:

1. `<project>/.jcode/agents/`
2. `<project>/.claude/agents/`
3. `~/.jcode/agents/`
4. `~/.claude/agents/`

Project directories are scanned from the session working directory up to the
project root, closest first. Scans are recursive and identity comes from `name`,
not the file name. A file without `name` is treated as documentation and
skipped silently. A missing `description`, an invalid `name` or bad YAML skips
the file with one warning. Unknown frontmatter keys are ignored.

The `agent` property in the swarm tool schema is a plain string. It describes
the user-level definitions only, because the tool schema is shared by every
session and cannot know which project a session works in. Project definitions
are looked up from the coordinator's working directory, never from the
`working_dir` the worker is sent to. An unknown name is an error that lists
the valid names.

## Identity

A worker spawned with a definition reports `agent_type` equal to the
definition's `name` in its hook payloads (see [HOOKS.md](HOOKS.md)). Without a
definition `agent_type` is the spawn label.

## Model, effort and tools

`model`, `effort`, `tools` and `disallowedTools` (also `disallowed-tools`) in a
definition are **operator configuration**: a person wrote the file, as with
Claude Code and Codex. The agent only picks *which* definition, so this does not
conflict with the rule that the swarm tool never lets an agent choose effort.

Order, highest first:

1. `[agents] force_swarm_defaults = true` (or `JCODE_SWARM_FORCE_DEFAULTS=1`):
   `swarm_model`, `swarm_effort` and your own tool policy always win, and the
   definition's `model`, `effort` and `tools` are ignored
2. the definition's `model`, `effort`, `tools`
3. `[agents] swarm_model` and `swarm_effort`
4. the coordinator's model

The per-spawn `model` parameter still needs `swarm_model_selection` and the
allowlist, and loses to a definition's `model`. There is no per-spawn effort.

- `model`: a model id, `route:model` (for example `openai-api:gpt-5.5`), or
  `inherit` to use the coordinator's model even when `swarm_model` pins another.
- `effort`: `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`. An
  unknown value is logged and ignored.
- `tools` narrows the worker to those tools and `disallowedTools` removes some.
  The result is the global tool policy intersected with `tools`, minus
  `disallowedTools`, minus the worker block list. It never widens anything.
  Tool names use Claude's vocabulary (`Read`, `Write`, `Edit`, `Bash`, `Grep`,
  `Glob`, `WebFetch`, `WebSearch`, `Task`, `TodoWrite`) and map onto jcode tools.
  A specifier such as `Bash(git push *)` applies to the whole tool. Unknown
  names are ignored.
- Tools are enforced for headless and inline workers. Headed (visible) workers
  attach from a separate process, so only `model` and `effort` apply to them and
  a warning is logged.

## Trust

Project definitions, including those in a project's `.claude/agents`, apply
their `model`, `effort` and `tools` only when the `agents` scope of the
[project trust](PROJECT_TRUST.md) store holds the exact digest of those policy
fields. Otherwise the worker gets the persona only, with operator defaults, and
one log line says so. Review with `jcode trust show` and approve with
`jcode trust grant --scope agents`. The digest covers each definition's name,
`model`, `effort`, `tools` and `disallowedTools` and not the persona text, which
is prompt-only like skills and `AGENTS.md`. Editing the policy needs approval
again, editing the persona does not. User-level definitions need no trust.
