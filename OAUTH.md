# OAuth Notes: Claude Code (Claude Max) + OpenAI/Codex

This document explains how OAuth works in J-Code, how to log in, and the one
detail that makes Claude Code tokens accept requests.

## Overview

J-Code uses OAuth tokens instead of API keys. Tokens are stored locally:
- Claude Max: `~/.jcode/auth.json`
- OpenAI/Codex: `~/.codex/auth.json`

Relevant code:
- Claude login + refresh: `src/auth/oauth.rs`
- Claude requests: `src/provider/claude.rs`
- OpenAI login + refresh: `src/auth/oauth.rs`
- OpenAI credentials parsing: `src/auth/codex.rs`
- OpenAI requests: `src/provider/openai.rs`

## Claude Code OAuth (Claude Max)

### Login steps
1. Run `jcode login --provider claude`.
2. Open the printed URL, click **Authorize**, then copy the code in the form
   `code#state` into the CLI prompt.
3. Verify with `jcode --provider claude run "Say hello from jcode"`.

Tokens are saved to `~/.jcode/auth.json`.

### The key detail that makes tokens work
Claude Code OAuth tokens are accepted only if the request body matches Claude
Code's system format. The fix is to send the Claude Code identifier as a
separate system block, not concatenated into the same string as the rest of the
system prompt.

Implementation: `src/provider/claude.rs:350`

Conceptually:
```json
{
  "system": [
    { "type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude." },
    { "type": "text", "text": "<your real system prompt>" }
  ]
}
```

J-Code also mirrors Claude Code headers:
- `anthropic-beta: oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14`
- `User-Agent: claude-code/2.0.76`

### Troubleshooting
- Error: `This credential is only authorized for use with Claude Code`
  - Re-run `jcode login --provider claude`.
  - Confirm the system blocks are separate as described above.
- Error: `OAuth token has been revoked`
  - Token is stale; re-login.

### Optional automation
If Firefox Agent Bridge is installed, you can automate the authorization click
and code capture for the login flow. Otherwise the manual flow above is fine.

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
