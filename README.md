# J-Code

A Rust coding agent that uses your existing **Claude Max** or **ChatGPT Pro** subscriptions via OAuth.

## Features

- **No API keys needed** - Uses OAuth tokens from Claude Code and Codex CLI
- **Dual provider support** - Works with both Anthropic Claude and OpenAI
- **Streaming responses** - Real-time output as the model generates
- **Built-in tools** - bash, read, write, edit, glob, grep, ls

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
# Auto-detect provider (tries Claude first, then OpenAI)
jcode

# Specify provider explicitly
jcode --provider claude
jcode --provider openai

# Run with initial prompt
jcode -m "Create a hello world program in Python"

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
| `glob` | Find files by pattern |
| `grep` | Search file contents with regex |
| `ls` | List directory contents |

## How It Works

J-Code reads OAuth credentials from:
- `~/.claude/.credentials.json` (Claude Max)
- `~/.codex/auth.json` (ChatGPT Pro/Plus)

It then uses these tokens to make API calls to the respective providers, just like the official CLI tools do.

## License

MIT
