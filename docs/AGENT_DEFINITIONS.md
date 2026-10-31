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

The `agent` property in the swarm tool schema lists the loaded names with their
descriptions. An unknown name is an error that lists the valid names.

## Identity

A worker spawned with a definition reports `agent_type` equal to the
definition's `name` in its hook payloads (see [HOOKS.md](HOOKS.md)). Without a
definition `agent_type` is the spawn label.

## Frontmatter fields read for policy

`model`, `effort`, `tools`, `disallowedTools` (also `disallowed-tools`) are
parsed here and applied as described in the model, effort and tools section
once that feature is enabled in your build. Tool names use Claude's vocabulary
(`Read`, `Bash`, `Grep`, `Task`, ...) and map onto jcode tools. A specifier such
as `Bash(git push *)` applies to the whole tool, as in Claude Code. Unknown tool
names are ignored.
