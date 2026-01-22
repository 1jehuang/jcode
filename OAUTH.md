# Auth Notes: Claude Code CLI + OpenAI/Codex

This document explains how authentication works in J-Code.

## Overview

J-Code uses the Claude Code CLI for Claude, and OAuth for OpenAI.

Credentials are stored locally:
- Claude Code CLI: `~/.claude/.credentials.json`
- OpenCode (optional): `~/.local/share/opencode/auth.json`
- OpenAI/Codex: `~/.codex/auth.json`

Relevant code:
- Claude provider: `src/provider/claude.rs`
- OpenAI login + refresh: `src/auth/oauth.rs`
- OpenAI credentials parsing: `src/auth/codex.rs`
- OpenAI requests: `src/provider/openai.rs`

## Claude Code CLI (Claude Max)

### Login steps
1. Install the Claude Code CLI.
2. Run `claude` (or `claude setup-token`) and complete login.
3. Verify with `jcode --provider claude run "Say hello from jcode"`.

J-Code does **not** store Claude OAuth tokens; it relies on the Claude Code CLI
credentials in `~/.claude/.credentials.json` (or OpenCode credentials, if present).

### Configuration knobs
These environment variables control the Claude Code CLI provider:
- `JCODE_CLAUDE_CLI_PATH` (default: `claude`)
- `JCODE_CLAUDE_CLI_MODEL` (default: `claude-opus-4-5-20251101`)
- `JCODE_CLAUDE_CLI_PERMISSION_MODE` (default: `bypassPermissions`)
- `JCODE_CLAUDE_CLI_PARTIAL` (set to `0` to disable partial streaming)

### Direct Anthropic API (optional)
Set `JCODE_USE_DIRECT_API=1` to bypass the CLI and use the Anthropic Messages API.
This requires tokens that Anthropic permits for direct API access (API keys, or
OAuth tokens explicitly allowed for API usage).

#### Claude OAuth direct API compatibility
Claude Code OAuth tokens can be used directly against the Messages API, but only
if the request matches the Claude Code "OAuth contract". jcode handles this
automatically when `JCODE_USE_DIRECT_API=1` and Claude OAuth credentials are
present.

Required behaviors (applied by the Anthropic provider):
- Use the Messages endpoint with `?beta=true`.
- Send `User-Agent: claude-cli/1.0.0`.
- Send `anthropic-beta: oauth-2025-04-20,claude-code-20250219`.
- Prepend the system blocks with the Claude Code identity line as the first
  block:
  - `You are Claude Code, Anthropic's official CLI for Claude.`

Tool name allow-list:
Claude OAuth requests reject certain tool names. jcode remaps tool names on the
wire and maps them back on responses so native tools continue to work. The
mapping is:
- `bash` → `shell_exec`
- `read` → `file_read`
- `write` → `file_write`
- `edit` → `file_edit`
- `glob` → `file_glob`
- `grep` → `file_grep`
- `task` → `task_runner`
- `todoread` → `todo_read`
- `todowrite` → `todo_write`

Notes:
- If the OAuth token expires, refresh via the Claude OAuth refresh endpoint.
- Without the identity line and allow-listed tool names, the API will reject
  OAuth requests even if the token is otherwise valid.

## OpenAI / Codex OAuth

### Login steps
1. Run `jcode login --provider openai`.
2. Your browser opens to the OpenAI OAuth page. The local callback listens on
   `http://localhost:9876/callback`.
3. After login, tokens are saved to `~/.codex/auth.json`.

### Request details
J-Code uses the Responses API. If you have a ChatGPT subscription (refresh
token or id_token present), requests go to:
- `https://chatgpt.com/backend-api/codex/responses`
with headers:
- `originator: codex_cli_rs`
- `chatgpt-account-id: <from token>`

Otherwise it uses:
- `https://api.openai.com/v1/responses`

### Troubleshooting
- 401/403: re-run `jcode login --provider openai`.
- Callback issues: make sure port 9876 is free and the browser can reach
  `http://localhost:9876/callback`.
