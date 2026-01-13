# J-Code

A Rust coding agent that uses the official **Claude Agent SDK** (Claude Code) or **ChatGPT Pro** via OAuth.

## Features

- **No API keys needed** - Uses Claude Code CLI credentials and Codex OAuth
- **Dual provider support** - Works with Claude Agent SDK and OpenAI/Codex
- **Streaming responses** - Real-time output as the model generates
- **Server/Client architecture** - Run as daemon, connect from multiple clients
- **12 built-in tools** - File ops, search, web, shell, and parallel execution

## Prerequisites

You need at least one of:
- **Claude Max subscription** - Install the SDK: `pip install claude-agent-sdk`, then run `claude` to authenticate
- **ChatGPT Pro/Plus subscription** - Run `codex login` to authenticate

## Installation

```bash
cargo install --path .
```

Or build from source:
```bash
cargo build --release
./target/release/jcode
```

## Usage

```bash
# Interactive REPL (default)
jcode

# Run a single command
jcode run "Create a hello world program in Python"

# Start as background server
jcode serve

# Connect to running server
jcode connect

# Specify provider explicitly
jcode --provider claude
jcode --provider openai

# Change working directory
jcode -C /path/to/project
```

## Tools

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands |
| `read` | Read file contents with line numbers |
| `write` | Create or overwrite files |
| `edit` | Edit files by replacing text |
| `multiedit` | Apply multiple edits to one file |
| `patch` | Apply unified diff patches |
| `glob` | Find files by pattern |
| `grep` | Search file contents with regex |
| `ls` | List directory contents |
| `webfetch` | Fetch URL content |
| `websearch` | Search the web (DuckDuckGo) |
| `codesearch` | Search code/documentation via Exa |
| `skill` | Load a skill from SKILL.md |
| `task` | Run a delegated sub-task |
| `todowrite` | Update todo list |
| `todoread` | Read todo list |
| `lsp` | LSP operations (fallback in jcode) |
| `invalid` | Report invalid tool calls |
| `batch` | Execute up to 10 tools in parallel |
| `mcp` | Manage MCP server connections |

## MCP (Model Context Protocol)

jcode supports MCP servers, allowing you to extend its capabilities with external tools.

### Configuration

Add MCP servers to `.claude/mcp.json` (project-local) or `~/.claude/mcp.json` (global):

```json
{
  "servers": {
    "playwright": {
      "command": "npx",
      "args": ["@anthropic/mcp-playwright"],
      "env": {}
    },
    "filesystem": {
      "command": "/path/to/mcp-filesystem",
      "args": ["--root", "/home/user"],
      "env": {}
    }
  }
}
```

Servers are automatically connected on startup and their tools are available with the prefix `mcp__servername__toolname`.

### Managing MCP Servers

The agent can manage MCP servers at runtime using the `mcp` tool:

```
# List connected servers and their tools
{"action": "list"}

# Connect to a new server
{"action": "connect", "server": "my-server", "command": "npx", "args": ["@some/mcp-server"]}

# Disconnect from a server
{"action": "disconnect", "server": "my-server"}

# Reload from config file
{"action": "reload"}
```

### Writing MCP Servers

MCP servers communicate via JSON-RPC 2.0 over stdio. See the [MCP specification](https://modelcontextprotocol.io/) for details on implementing your own servers.

## Architecture

```
┌─────────────────────────────────────────────┐
│              CLI / Client                   │
├─────────────────────────────────────────────┤
│         Server (Unix Socket)                │
├─────────────────────────────────────────────┤
│              Agent Loop                     │
├─────────────────────────────────────────────┤
│            Provider Trait                   │
│  ┌──────────────┐  ┌──────────────┐        │
│  │ Claude Agent │  │ OpenAI/Codex │        │
│  │     SDK      │  │    OAuth     │        │
│  └──────────────┘  └──────────────┘        │
├─────────────────────────────────────────────┤
│              Tool System                    │
│  bash │ read │ write │ edit │ glob │ ...   │
└─────────────────────────────────────────────┘
```

## How It Works

J-Code uses Claude Agent SDK to talk to Claude Code. Claude Code credentials are stored at:
- `~/.claude/.credentials.json` (Claude Code CLI)
- `~/.local/share/opencode/auth.json` (OpenCode, if installed)

OpenAI/Codex OAuth credentials are still stored at:
- `~/.codex/auth.json`

For provider/auth details, see `OAUTH.md`.

## Testing

- `cargo test`
- `cargo run --bin test_api` (Claude Agent SDK smoke test)
- `cargo run --bin jcode-harness` (tool harness; add `--include-network` to exercise web tools)
- `scripts/agent_trace.sh` (end-to-end agent smoke test with trace logs; set `JCODE_PROVIDER=openai|claude`)

## License

MIT
