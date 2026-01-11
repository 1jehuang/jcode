# Auth Notes: Claude Agent SDK + OpenAI/Codex

This document explains how authentication works in J-Code.

## Overview

J-Code uses the official Claude Agent SDK for Claude, and OAuth for OpenAI.

Credentials are stored locally:
- Claude Code CLI: `~/.claude/.credentials.json`
- OpenCode (optional): `~/.local/share/opencode/auth.json`
- OpenAI/Codex: `~/.codex/auth.json`

Relevant code:
- Claude Agent SDK bridge: `scripts/claude_agent_sdk_bridge.py`
- Claude provider: `src/provider/claude.rs`
- OpenAI login + refresh: `src/auth/oauth.rs`
- OpenAI credentials parsing: `src/auth/codex.rs`
- OpenAI requests: `src/provider/openai.rs`

## Claude Agent SDK (Claude Max)

### Login steps
1. Install the SDK: `pip install claude-agent-sdk`.
2. Run `claude` (or `claude setup-token`) and complete login.
3. Verify with `jcode --provider claude run "Say hello from jcode"`.

J-Code does **not** store Claude OAuth tokens anymore; it relies on the Claude
Code CLI credentials used by the SDK.

### Configuration knobs
These environment variables control the SDK bridge:
- `JCODE_CLAUDE_SDK_PYTHON` (default: `python3`)
- `JCODE_CLAUDE_SDK_MODEL` (default: `claude-sonnet-4-20250514`)
- `JCODE_CLAUDE_SDK_PERMISSION_MODE` (default: `bypassPermissions`)
- `JCODE_CLAUDE_SDK_CLI_PATH` (optional, custom `claude` binary)
- `JCODE_CLAUDE_SDK_SCRIPT` (optional, custom bridge path)
- `JCODE_CLAUDE_SDK_PARTIAL` (set to `0` to disable partial streaming)

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
