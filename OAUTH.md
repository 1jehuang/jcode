# Auth Notes: Claude Code CLI + OpenAI/Codex + Gemini

This document explains how authentication works in J-Code.

## Overview

J-Code auto-imports existing local credentials and can also run built-in OAuth login flows.

Credentials are stored locally:
- J-Code Claude OAuth (if logged in via `jcode login --provider claude`): `~/.jcode/auth.json`
- Claude Code CLI: `~/.claude/.credentials.json`
- OpenCode (optional): `~/.local/share/opencode/auth.json`
- J-Code OpenAI/Codex OAuth: `~/.jcode/openai-auth.json`
- Codex CLI import/migration source: `~/.codex/auth.json`
- Gemini native OAuth: `~/.jcode/gemini_oauth.json`
- Gemini CLI import fallback: `~/.gemini/oauth_creds.json`

Relevant code:
- Claude provider: `src/provider/claude.rs`
- OpenAI login + refresh: `src/auth/oauth.rs`
- OpenAI credentials parsing: `src/auth/codex.rs`
- OpenAI requests: `src/provider/openai.rs`
- Azure OpenAI auth/config: `src/auth/azure.rs`
- Azure OpenAI transport: `src/provider/openrouter.rs`
- Gemini login + refresh: `src/auth/gemini.rs`
- Gemini Code Assist provider: `src/provider/gemini.rs`

## Claude (Claude Max)

### Login steps
1. Run `jcode login --provider claude` (recommended), or `jcode login` and choose Claude.
2. Alternative: run `claude` (or `claude setup-token`) and J-Code will auto-import `~/.claude/.credentials.json`.
3. Verify with `jcode --provider claude run "Say hello from jcode"`.

Credential discovery order is:
1. `~/.jcode/auth.json`
2. `~/.claude/.credentials.json`
3. `~/.local/share/opencode/auth.json`

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
3. After login, tokens are saved to `~/.jcode/openai-auth.json`.

Credential discovery order is:
1. `~/.jcode/openai-auth.json`
2. `~/.codex/auth.json`
3. `OPENAI_API_KEY`

If jcode imports OAuth tokens from `~/.codex/auth.json`, it moves those tokens
into `~/.jcode/openai-auth.json` and clears the legacy OAuth token from the
Codex file. A legacy `OPENAI_API_KEY` entry, if present, is preserved.

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
- Claude 401/auth errors: run `jcode login --provider claude`.
- 401/403: re-run `jcode login --provider openai`.
- Callback issues: make sure port 9876 is free and the browser can reach
  `http://localhost:9876/callback`.

## Azure OpenAI

This was added after comparing J-Code to OpenCode/Crush. The meaningful auth gap
was not another browser OAuth flow, but support for **Azure OpenAI** using either:
- **Microsoft Entra ID** credentials (via Azure's `DefaultAzureCredential` chain), or
- **Azure OpenAI API keys**.

### Login/setup steps
1. Run `jcode login --provider azure`.
2. Enter your Azure OpenAI endpoint, for example:
   - `https://your-resource.openai.azure.com`
3. Enter your Azure deployment/model name.
4. Choose one auth mode:
   - **Entra ID** (recommended)
   - **API key**
5. jcode saves settings to `~/.config/jcode/azure-openai.env`.

### Stored configuration
The Azure env file may contain:
- `AZURE_OPENAI_ENDPOINT`
- `AZURE_OPENAI_MODEL`
- `AZURE_OPENAI_USE_ENTRA`
- `AZURE_OPENAI_API_KEY` (only when using key auth)

### Runtime behavior
- jcode normalizes the endpoint to the newer Azure OpenAI `/openai/v1` base.
- In **Entra ID** mode, jcode obtains bearer tokens using `azure_identity::DefaultAzureCredential` with scope:
  - `https://cognitiveservices.azure.com/.default`
- In **API key** mode, jcode sends the credential in the Azure-style `api-key` header.
- The Azure provider currently reuses J-Code's OpenAI-compatible transport layer under the hood.
- Model catalog fetching is disabled for Azure by default, so you should configure a deployment/model explicitly.

### Entra ID credential sources
`DefaultAzureCredential` can resolve credentials from sources like:
- `az login`
- managed identity
- Azure environment credentials

### Troubleshooting
- If Entra ID auth fails locally, try `az login` first.
- Make sure your identity has access to the Azure OpenAI resource.
- If requests fail with deployment/model errors, verify `AZURE_OPENAI_MODEL` matches your deployed model name.
- If you prefer static credentials, re-run `jcode login --provider azure` and choose API key mode.

## Gemini OAuth

### Login steps
1. Run `jcode login --provider gemini` or `/login gemini` inside the TUI.
2. jcode opens a browser to the Google OAuth flow used for Gemini Code Assist.
3. If local callback binding is unavailable, jcode falls back to a manual paste flow using `https://codeassist.google.com/authcode`.
4. Tokens are saved to `~/.jcode/gemini_oauth.json`.

### Credential discovery order
1. Native jcode Gemini tokens: `~/.jcode/gemini_oauth.json`
2. Imported Gemini CLI OAuth tokens: `~/.gemini/oauth_creds.json`

### Runtime notes
- jcode uses native Google OAuth and talks to the Google Code Assist backend directly.
- Expired tokens are refreshed automatically using the Google refresh token.
- Some school / Workspace accounts may require `GOOGLE_CLOUD_PROJECT` or `GOOGLE_CLOUD_PROJECT_ID` for Code Assist entitlement checks.

### Troubleshooting
- If browser launch fails, set `NO_BROWSER=true` and use the pasted callback/code flow.
- If entitlement or onboarding fails for a Workspace account, set `GOOGLE_CLOUD_PROJECT` and retry.
- If login succeeds but requests fail later, re-run `jcode login --provider gemini` to refresh the stored session.

## Experimental CLI Providers

J-Code also supports experimental CLI-backed providers:
- `--provider cursor`
- `--provider copilot`
- `--provider antigravity`

These use each provider's local CLI session/auth and shell out in print mode.

### Cursor
- Login: `jcode login --provider cursor`
  - fast path: runs `cursor-agent login`
  - fallback: saves `CURSOR_API_KEY` to `~/.config/jcode/cursor.env`
- Runtime:
  - jcode shells out to `cursor-agent`
  - if a Cursor API key is configured, jcode injects it via `CURSOR_API_KEY`
  - `cursor-agent status` is used to probe whether a local CLI session is authenticated
- Env vars:
  - `JCODE_CURSOR_CLI_PATH` (default: `cursor-agent`)
  - `JCODE_CURSOR_MODEL` (default: `gpt-5`)
  - `CURSOR_API_KEY` (optional; overrides saved key)

### GitHub Copilot
- Login: `jcode login --provider copilot` (runs `copilot -i /login`, or `gh copilot -- -i /login` if `copilot` is not on PATH)
- Env vars:
  - `JCODE_COPILOT_CLI_PATH` (optional override for CLI path)
  - `JCODE_COPILOT_MODEL` (default: `claude-sonnet-4`)

### Antigravity
- Login: `jcode login --provider antigravity` (runs `<cli> login`)
- Env vars:
  - `JCODE_ANTIGRAVITY_CLI_PATH` (default: `antigravity`)
  - `JCODE_ANTIGRAVITY_MODEL` (default: `default`)
  - `JCODE_ANTIGRAVITY_PROMPT_FLAG` (default: `-p`)
  - `JCODE_ANTIGRAVITY_MODEL_FLAG` (default: `--model`)
