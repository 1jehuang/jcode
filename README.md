# J-Code

A Rust coding agent that uses your existing **Claude Max** or **ChatGPT Pro** subscriptions via OAuth.

## Features

- **No API keys needed** - Uses OAuth tokens from Claude Code and Codex CLI
- **Dual provider support** - Works with both Anthropic Claude and OpenAI
- **Streaming responses** - Real-time output as the model generates
- **Server/Client architecture** - Run as daemon, connect from multiple clients
- **11 built-in tools** - File ops, search, web, and shell commands

## Prerequisites

You need at least one of:
- **Claude Max subscription** - Run `claude` and `/login` to authenticate
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
│  │ Claude Max   │  │ OpenAI/Codex │        │
│  │    OAuth     │  │    OAuth     │        │
│  └──────────────┘  └──────────────┘        │
├─────────────────────────────────────────────┤
│              Tool System                    │
│  bash │ read │ write │ edit │ glob │ ...   │
└─────────────────────────────────────────────┘
```

## How It Works

J-Code reads OAuth credentials from:
- `~/.claude/.credentials.json` (Claude Max)
- `~/.codex/auth.json` (ChatGPT Pro/Plus)

It then uses these tokens to make API calls to the respective providers, just like the official CLI tools do.

## License

MIT
