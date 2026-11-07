# Claude Code `/mcp` and `/skills`: captured reference (v2.1.302)

Captured live from Claude Code 2.1.302 in tmux (read-only navigation), with
strings cross-checked in the 2.1.301 bundle (`m1390.js` approval dialog,
`m2107.js` skills dialog). This is the target experience for jcode.

## `/mcp`

Root (`↑/↓ to navigate · Enter to confirm · Esc to cancel`):

```
Manage MCP servers
7 servers
  User MCPs (/Users/<you>/.claude.json)
❯ ✔ claude-peers             4 tools
  ✘ codebase-memory-mcp
  ✔ phone                    12 tools
  ✔ rea                      207 tools
  claude.ai
  ✔ claude.ai Claude Docs    8 tools
  ⚠ claude.ai Google Drive   needs authentication
  Built-in MCPs (always available)
  ◯ computer-use
※ Run claude --debug to see error logs
https://code.claude.com/docs/en/mcp for help
```

Server page (`↑/↓ to navigate · Enter to select · Esc to back`); actions are
numbered and depend on state:

```
Claude-peers MCP Server                 Codebase-memory-mcp MCP Server
Status:           ✔ connected           Status:           ✘ failed
Command:          <command + args>      Command:          <command>
Config location:  /Users/<you>/.claude.json
Capabilities: tools                     ❯ 1. Reconnect
Tools: 4 tools                            2. Disable
❯ 1. View tools
  2. Reconnect                          Claude.ai Google Drive MCP Server
  3. Disable                            Status:           ⚠ needs authentication
                                        URL:              https://…
                                        Config location:  claude.ai
                                        ❯ 1. Authenticate
                                          2. Disable
```

A disabled server shows `◯ disabled` and only `1. Enable`. After Enable or
Disable the dialog returns to the server list. Enable/disable is stored per
project (`projects[<path>].enabledMcpServers` / `disabledMcpServers`).

Tools list (`Tools for <server>`, `N tools`), one row per tool, with MCP
annotation hints on the right (`destructive · open-world`, `read-only`).
Typing does not filter. Tool page:

```
list_peers
claude-peers
Tool name: list_peers
Full name: mcp__claude-peers__list_peers
Description:
<description>
Parameters:
  ● scope (required): string - <description>
Esc to go back
```

MCP prompts appear in the `/` menu as `/<server>:<prompt> (MCP)`.

Project `.mcp.json` approval (found walking up parent directories), shown at
startup before the server starts:

```
New MCP server found in this project: <name>
MCP servers may execute code or access system resources. All tool calls require approval. Learn more in the MCP documentation.
  Use this MCP server
  Use this and all future MCP servers in this project
❯ Continue without using this MCP server
Enter to confirm · Esc to cancel
```

Esc means "no". Answers are saved in the project's `.claude/settings.local.json`
(`enabledMcpjsonServers`, `disabledMcpjsonServers`, `enableAllProjectMcpServers`).

## `/skills`

```
Skills
33 skills · enter/space to cycle, / to search, t to sort, Esc to close
╭──────────────────────────╮
│ ⌕ Search skills…         │
╰──────────────────────────╯
❯ ✔ anthropic-skills:deep-research   claude.ai sync · ~200 tok
  ✔ claudex-loop:claudex-loop        locked by plugin · plugin · ~580 tok
  ✔ setupdotclaude:setupdotclaude    locked by author · user-only · plugin · ~60 tok
  ✔ amq-cli                          user · ~300 tok
  ↓ 1 more
Plugin skills are managed via /plugin
```

- Modes cycle on → name-only → user-only → off (✔ for the first three, ◯ for off);
  the mode word is shown only when not `on`.
- Row meta order: lock reason (`locked by plugin`, `locked by author`,
  `reserved name`), mode word, source, `~N tok`.
- `t` sorts by tokens (subtitle gains `· sorted by tokens`); `/` focuses search
  (`type to filter · ↓/enter to select · esc to clear`).
- Saved on close to the project's `.claude/settings.local.json` under
  `skillOverrides`; the transcript shows `Updated N skill overrides` or
  `No changes`.
- Plugin skills are named `plugin:skill`.
