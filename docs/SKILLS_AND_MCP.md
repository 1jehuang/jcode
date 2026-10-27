# Skills and MCP servers

jcode follows Claude Code here: the same skill folders, the same `SKILL.md`
format, the same `/skills` and `/mcp` screens, and the same MCP config files.
Anything you set up for Claude Code works in jcode without copying.

## Skills

A skill is a folder with a `SKILL.md`: YAML frontmatter, then instructions.

```markdown
---
name: deploy                      # optional, defaults to the folder name
description: Deploy the current branch to staging. Use when asked to ship.  # optional: defaults to the first line of the body
disable-model-invocation: true    # optional: only you can run it, via /deploy
allowed-tools: Bash, Read         # optional, shown in skill details
---

Deploy $ARGUMENTS. Scripts live in ${CLAUDE_SKILL_DIR}/scripts.
```

Where jcode finds skills, later entries winning on a name clash:

| Location | Notes |
|---|---|
| Claude Code plugins (`~/.claude/plugins`) | Named `plugin:skill`, e.g. `/ponytail:ponytail`. A plain `/ponytail` also works when only one plugin has that skill. Plugins turned off in Claude Code (`enabledPlugins`) are skipped. |
| `~/.jcode/skills/` | jcode's own global skills. |
| `~/.claude/skills/` | Read live: skills you add in Claude Code show up in jcode. |
| `~/.agents/skills/` | The shared cross-tool folder. |
| `<project>/.jcode/skills/`, `.agents/skills/`, `.claude/skills/` | Project skills, for sessions in that project. |

How a skill is used:

- **You run it** with `/name` or `/name some arguments`. The skill is added to
  the conversation once, with its folder path and `$ARGUMENTS` filled in. Your
  arguments are also sent as your message.
- **The model runs it itself** when your request matches a listed skill's
  description. It loads the skill with the `skill_manage` tool (called `Skill`
  on some providers).

The model sees only each skill's name and description until it loads one.

### `/skills`

Lists every skill with its source and roughly how many tokens it adds to the
model's skill list. Type to search, `t` to sort by tokens, Enter or Space to
cycle a skill through:

| Mode | Effect |
|---|---|
| on | Listed with its description; you and the model can run it. |
| name-only | Listed without its description (cheaper); both can still run it. |
| user-only | Not listed; only you can run it with `/name`. |
| off | Not listed and cannot be run. |

Changes are saved when you close the dialog, to `~/.jcode/skill-overrides.json`,
and apply to new sessions (the current session keeps its cached skill list).
Skills marked `locked by author` set `disable-model-invocation`; plugin skills
are `locked by plugin`, since they are managed with Claude Code's `/plugin`.

## MCP servers

jcode reads servers from, in increasing precedence:

1. `~/.jcode/mcp.json`
2. `~/.claude.json` (`mcpServers`, and `projects["<path>"].mcpServers`)
3. `~/.claude/mcp.json`
4. The project's `.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json`

`${VAR}` and `${VAR:-default}` are expanded in commands, arguments,
environment, URLs and headers. stdio, streamable HTTP and SSE are supported.

### Project servers need approval

A server defined in a project's own config file can come from any repository
you clone, so it does not start until you approve it. When a session starts in
such a project, jcode asks:

```
New MCP server found in this project: <name>
  Use this MCP server
  Use this and all future MCP servers in this project
❯ Continue without using this MCP server
```

Esc means "continue without". Until you approve it, nothing starts the
server: not startup, not a tool call from the model, not reconnect. The same
holds for any server you disable in `/mcp`. Answers are remembered per project in
`~/.jcode/mcp-approvals.json`, outside the project, so a repository cannot
approve its own servers. Setting `"enableAllProjectMcpServers": true` in your
own `~/.claude/settings.json` approves every project server.

### `/mcp`

Shows servers grouped by config file, with status (`✔` connected, `✘` failed,
`△` needs approval, `◯` disabled) and tool count. Open a server for its
command or URL, config location, the connection error if it failed, and
numbered actions (press the digit or Enter):

- **View tools**: each tool with its `read-only` / `destructive` /
  `open-world` hints, then its full name, description and parameters.
- **Reconnect**: closes the dialog and reports the result.
- **Disable / Enable**: saved in `~/.jcode/mcp-enabled.json` (jcode never
  rewrites Claude Code's files). For a project server this is the same as
  rejecting or approving it.

You can also type `/mcp reconnect|enable|disable|approve|reject <server>`.

A server that needs sign-in: run `jcode mcp login <server>` in a terminal,
then **Reconnect**.

### What the model sees

The model is told the names of available MCP tools, not their schemas. It
loads the ones it needs with `mcp_search` (`select:<name>`); a server's own
usage instructions come with them. It is also told when a server failed to
connect or is waiting for your approval, so it can tell you instead of
guessing.
