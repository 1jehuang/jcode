# Plan: Dynamic Skills and MCP Support

## Goals
1. Hot-reload skills without restart
2. MCP (Model Context Protocol) server support
3. Dynamic tool registration at runtime
4. Agent can add/configure MCP servers itself

## Current State
- Skills: Loaded from `~/.claude/skills/` and `./.claude/skills/` at startup
- Tools: Hardcoded in `Registry::new()`
- No MCP support

---

## Implementation Plan

### Phase 1: Hot-reload Skills

**Changes to `src/skill.rs`:**
- Add `reload(&mut self)` method to `SkillRegistry`
- Skills can be reloaded without restarting

**New tool `reload_skills`:**
- Agent can trigger `reload_skills` to pick up new skills

### Phase 2: Dynamic Tool Registry

**Changes to `src/tool/mod.rs`:**
```rust
impl Registry {
    /// Register a new tool at runtime
    pub async fn register(&self, tool: Arc<dyn Tool>);

    /// Unregister a tool by name
    pub async fn unregister(&self, name: &str);

    /// List all registered tools
    pub async fn list(&self) -> Vec<String>;
}
```

### Phase 3: MCP Client

**New module `src/mcp/mod.rs`:**
- MCP protocol types (JSON-RPC 2.0)
- MCP client for stdio-based servers
- MCP tool wrapper (converts MCP tools to our Tool trait)

**Config file `~/.claude/mcp.json`:**
```json
{
  "servers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-server-filesystem", "/path"],
      "env": {}
    },
    "github": {
      "command": "npx",
      "args": ["-y", "@anthropic/mcp-server-github"],
      "env": {"GITHUB_TOKEN": "..."}
    }
  }
}
```

**MCP Manager:**
- Load config on startup
- Connect to configured servers
- Convert MCP tools to jcode Tool trait
- Handle server lifecycle (start, stop, restart)

### Phase 4: Agent Self-Configuration

**New tools:**
- `mcp_list` - List connected MCP servers
- `mcp_connect` - Start a new MCP server
- `mcp_disconnect` - Stop an MCP server
- `mcp_reload` - Reload all MCP servers

**Flow:**
1. Agent calls `mcp_connect {"name": "playwright", "command": "npx", "args": ["-y", "@anthropic/mcp-server-playwright"]}`
2. jcode spawns the process, does MCP handshake
3. Tools from server are added to registry
4. Agent can immediately use the new tools

---

## MCP Protocol Summary

MCP uses JSON-RPC 2.0 over stdio:

**Initialize:**
```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"jcode","version":"0.1.0"}}}
```

**List tools:**
```json
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
```

**Call tool:**
```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"/tmp/test.txt"}}}
```

---

## Files to Create/Modify

1. `src/mcp/mod.rs` - MCP module
2. `src/mcp/protocol.rs` - JSON-RPC types
3. `src/mcp/client.rs` - MCP client
4. `src/mcp/manager.rs` - Multi-server manager
5. `src/mcp/tool.rs` - MCP tool wrapper
6. `src/tool/mod.rs` - Add dynamic registration
7. `src/tool/mcp_tools.rs` - mcp_connect, mcp_list, etc.
8. `src/skill.rs` - Add reload()
9. `src/tool/reload_skills.rs` - reload_skills tool

## Order of Implementation
1. Dynamic tool registry (prerequisite)
2. Skill hot-reload (quick win)
3. MCP protocol types
4. MCP client (single server)
5. MCP manager (multi-server)
6. MCP tools for agent self-config
