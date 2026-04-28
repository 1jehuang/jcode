# MCP and Dynamic Skills Status

Status: Implemented baseline, active refinement
Updated: 2026-04-28

This page used to be the implementation plan for MCP and dynamic skills. It is now a status page for the current implementation.

## Current state

jcode has first-class MCP client support and a dynamic tool registry:

- Skills can be loaded, listed, read, reloaded individually, or reloaded as a group through the `skill_manage` tool.
- The tool registry supports runtime registration and prefix unregistering, which is used by MCP reload/connect flows.
- MCP servers are configured separately from `config.toml`.
- MCP servers are connected in the background so startup is not blocked.
- MCP tools are registered as normal jcode tools with names like `mcp__<server>__<tool>`.
- The built-in `mcp` management tool supports `list`, `connect`, `disconnect`, and `reload`.
- In daemon/server mode, MCP servers marked `shared: true` use a shared server pool so multiple sessions do not spawn duplicate stateless server processes.
- Servers marked `shared: false` are spawned per session for stateful tools such as browser automation.

## Config files

Primary config files:

- `~/.jcode/mcp.json` for global MCP servers
- `.jcode/mcp.json` for project-local MCP servers

Compatibility/import sources:

- `.claude/mcp.json` is still read for project compatibility.
- On first run, if `~/.jcode/mcp.json` does not exist, jcode tries to import servers from `~/.claude/mcp.json` and `~/.codex/config.toml`.
- Codex `[mcp_servers.*]` tables are converted to jcode MCP server entries.

Example:

```json
{
  "servers": {
    "filesystem": {
      "command": "/path/to/mcp-server",
      "args": ["--root", "/workspace"],
      "env": {},
      "shared": true
    },
    "playwright": {
      "command": "npx",
      "args": ["-y", "@playwright/mcp"],
      "env": {},
      "shared": false
    }
  }
}
```

`shared` defaults to `true`.

## Runtime management tool

The management tool is named `mcp`.

```json
{"action": "list"}
```

```json
{
  "action": "connect",
  "server": "filesystem",
  "command": "/path/to/mcp-server",
  "args": ["--root", "/workspace"],
  "env": {}
}
```

```json
{"action": "disconnect", "server": "filesystem"}
```

```json
{"action": "reload"}
```

`reload` re-reads MCP config, unregisters existing `mcp__` tools, reconnects configured servers, and registers the newly discovered server tools.

## Protocol support

jcode acts as an MCP client over JSON-RPC 2.0 stdio:

1. Spawn the configured server command.
2. Send `initialize` with jcode client info.
3. Send `notifications/initialized`.
4. Call `tools/list`.
5. Wrap each MCP tool in jcode's `Tool` trait.
6. Execute tools through `tools/call`.

Current focus is tools. Resource and prompt protocol types exist, but the user-facing implementation is centered on tool discovery and tool calls.

## UI/server visibility

- The TUI receives `McpStatus` events and shows connecting/connected server state.
- Remote/client sessions receive configured MCP tool registration through the server.
- The debug socket exposes MCP inspection commands for testing.
- MCP connection failures are logged and surfaced through the management tool instead of silently disappearing.

## Important implementation files

- `src/mcp/protocol.rs` - JSON-RPC, MCP protocol, config loading/import
- `src/mcp/client.rs` - stdio MCP client
- `src/mcp/manager.rs` - per-session manager and shared-pool delegation
- `src/mcp/pool.rs` - shared MCP server process pool
- `src/mcp/tool.rs` - wrapper for discovered MCP server tools
- `src/tool/mcp.rs` - built-in MCP management tool
- `src/tool/mod.rs` - dynamic registry and MCP tool registration
- `src/server.rs` - server-side session registry wiring

## Open items

- Add richer docs for writing and testing custom MCP servers.
- Decide whether resource/prompt support should become user-facing or remain protocol scaffolding.
- Continue hardening reload/disconnect behavior around long-running or stateful MCP servers.
- Keep OpenAI Apps SDK compatibility separate from jcode's current MCP client unless a real Apps SDK server/descriptor layer is added.
