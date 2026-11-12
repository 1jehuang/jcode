# Project trust

Some files inside a repository make jcode run programs without you typing
anything. A project MCP file starts its servers, project hooks run on every
prompt and tool call, and project agent definitions can change a worker's
model, effort and tools. Anyone who can put files in a repository you clone can
write those files, so jcode asks once, remembers the answer **outside the
repository**, and asks again when the files change.

## Scopes

| Scope | What it covers | Where it comes from |
| --- | --- | --- |
| `mcp` | MCP servers that run a local command | `.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json` in the project |
| `hooks` | the `[hooks]` table of the project | `.jcode/config.toml` |
| `agents` | `model`, `effort`, `tools` of project agent definitions | `.jcode/agents`, `.claude/agents` (used by later changes) |

The `agents` scope is reserved here: until the change that uses it lands,
`trust grant --scope agents` has nothing to approve. Scopes are independent.
Granting one never grants another. Not gated: skills,
`AGENTS.md`, prompt overlays, global config and global MCP. Remote (HTTP/SSE)
MCP servers run no local command and are not gated.

## Commands

```bash
jcode trust show [path]                     # what the project wants to run, per scope: trusted, untrusted or changed
jcode trust grant [path] [--scope hooks|mcp|agents|all] [--yes]
jcode trust revoke [path] [--scope ...]
jcode trust list                            # every approved project and scope
```

`jcode mcp trust` and `jcode mcp revoke` remain as aliases for the `mcp` scope.
`grant` shows exactly what will be approved and asks `[y/N]`. `--yes` skips the
prompt for scripts, after you reviewed `jcode trust show`. If an environment
value had to be redacted from the review, inline approval is disabled and
`--yes` is required, so you review the underlying files first.

Interactive sessions offer to approve untrusted scopes at startup. Headless
runs (`jcode run`, swarm workers, the harness API) never prompt: untrusted
scopes are skipped and a log line names the command to run.

## What "the same" means

The store is `~/.jcode/state/project-trust.json` (`<JCODE_HOME>/state`),
versioned. If it cannot be read or has an unknown version, nothing is trusted
and it is never overwritten.

The project key is the git top level of the working directory. A linked git
worktree uses its **main checkout's** root, so one approval covers every
worktree, but the scope digest must still match: a worktree whose files differ
is untrusted.

Each scope stores `sha256:<hex>` of canonical JSON (recursively sorted keys,
compact separators) of that scope's content, so other tools can recompute it:

- `mcp`: per server, `command`, `args`, `env`, `shared`, `transport`, `url`,
  `headers`, `enabled`, `disabled`, after environment expansion.
- `hooks`, `agents`: described in `HOOKS.md` and `AGENT_DEFINITIONS.md`.
