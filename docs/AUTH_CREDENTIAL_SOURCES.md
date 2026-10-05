# Auth Credential Sources (single source of truth)

This document exists because the same confusion keeps recurring: an agent (or a
human) greps for `ANTHROPIC_API_KEY` / `sk-ant-api`, finds nothing, reads a stale
`auth-validation.json` entry that says "expired", and wrongly concludes "there is
no working credential". In reality the credential is present and working; it just
lives somewhere the naive search did not look.

If you are debugging "does provider X have a credential?", read this first.

## The two "dual-auth" providers: Anthropic and OpenAI

Anthropic/Claude and OpenAI each support **two completely independent credential
paths**, surfaced as **two separate login providers**:

| Concept            | Login provider id | Auth kind | Where the credential lives                                  |
|--------------------|-------------------|-----------|-------------------------------------------------------------|
| Claude, OAuth/sub  | `claude`          | OAuth     | `~/.jcode/auth.json` → `anthropic_accounts[].access` (`sk-ant-oat...`) |
| Claude, API key    | `anthropic-api`   | API key   | `ANTHROPIC_API_KEY` env **or** `~/.config/jcode/anthropic.env`         |
| OpenAI, OAuth      | `openai`          | OAuth     | `~/.jcode/openai-auth.json` (Codex/ChatGPT login)            |
| OpenAI, API key    | `openai-api`      | API key   | `OPENAI_API_KEY` env **or** `~/.config/jcode/openai.env`     |

Key facts that trip people up:

- The **OAuth token is not an API key.** Anthropic OAuth tokens are `sk-ant-oat01-...`
  (and refresh tokens `sk-ant-ort01-...`). A direct API key is `sk-ant-api03-...`.
  Grepping for `sk-ant-api` will miss an OAuth-only setup, and vice versa.
- The **API key is usually in the app config dir, not an env var.** The canonical
  store is `~/.config/jcode/anthropic.env` (XDG `$XDG_CONFIG_HOME/jcode/anthropic.env`),
  written by `jcode login --provider anthropic-api`. `printenv ANTHROPIC_API_KEY`
  returning nothing does **not** mean there is no key.
- `~/.jcode/auth.json` holds **only OAuth accounts**, never the API key.
- `claude` and `anthropic-api` are **different providers** with different
  availability. Having a Claude subscription login (OAuth) does **not** make
  `anthropic-api` usable, and vice versa.

To sign in through the installed Claude Code CLI instead of Jcode's OAuth
browser flow, run `jcode login --provider claude --claude-code` in an interactive
terminal on the machine running Jcode. When the `claude` binary is installed,
this is the default: `jcode login --provider claude` and the local `/login`
Claude choice (press Enter) both use the Claude Code CLI. Use
`jcode login --provider claude --oauth`, choose 1 in `/login`, or set
`JCODE_CLAUDE_LOGIN_METHOD=oauth` to keep Jcode's OAuth flow. Account labels,
`--no-browser`, scriptable flags, and non-interactive shells
always use Jcode's OAuth flow. Jcode asks permission before using Claude Code's login.
Claude Code retains its own credential; on macOS, Jcode copies its Keychain
credential into `~/.jcode/auth.json` so Jcode's direct API runtime can use it.
With a custom `CLAUDE_CONFIG_DIR` or `JCODE_HOME`, run the command in a shell
with the same environment as Jcode. The TUI will not open a new terminal in
that case because a new login shell can lose those settings.
Because both tools may refresh the same initial token, a copied login may need
to be renewed independently. Over SSH, run this command on the remote host;
the SSH login bridge does not transfer the CLI's credentials.
This method does not select a Jcode account label. Choose Jcode's OAuth login
for managed per-account defaults.

### How to actually check (don't guess)

```sh
# The honest, normalized answer for every provider:
jcode auth status --json
```

Each provider entry reports `status`, `auth_kind` ("OAuth" vs "API key"),
`credential_source` (env var / app config file / jcode-managed file), and the
exact `method`. This is the canonical surface; prefer it over grepping files.

Programmatically, the single source of truth is
`AuthStatus::assessment_for_provider(descriptor)` in
`crates/jcode-base/src/auth/mod.rs`, which returns a `ProviderAuthAssessment`.

## Selecting a default via config

`~/.jcode/config.toml`:

```toml
[provider]
default_provider = "claude"        # Claude subscription (OAuth)
# default_provider = "anthropic-api" # Claude via direct Anthropic API key
default_model = "claude-opus-4-8"
anthropic_reasoning_effort = "xhigh"
```

- `default_provider = "claude"` uses the OAuth/subscription credential.
- `default_provider = "anthropic-api"` uses the direct API key. In this mode the
  runtime **does not** fall back to OAuth: if no API key is configured the request
  fails. Make sure `~/.config/jcode/anthropic.env` (or `ANTHROPIC_API_KEY`) exists.

The full alias/vocabulary mapping (runtime env, route stable-id, CLI `--provider`,
model prefix) is centralized in
`crates/jcode-provider-core/src/auth_mode.rs` (`AuthRoute`). Do not re-parse these
strings by hand; go through `AuthRoute`.

## Why "expired" was misleading: validation cache is not live state

`~/.jcode/auth-validation.json` caches the result of the **last** runtime
auth-test per provider. It is a historical record, not the current credential
state. An OAuth token that has since auto-refreshed can still show a days-old
"validation failed / expired" entry here.

To avoid presenting stale records as current fact, `format_record_label`
(`crates/jcode-base/src/auth/validation.rs`) flags any record older than
`doctor::VALIDATION_STALE_AFTER_MS` (7 days) as `stale, ... re-validate`. Treat a
stale record as "unknown, re-check", never as ground truth. Re-validate with:

```sh
jcode auth-test --provider <id>
```

## Quick decision tree for "is provider X authenticated?"

1. Run `jcode auth status --json` and read the entry for the **specific** login
   provider id (`claude` vs `anthropic-api` are different!).
2. If you must inspect files: OAuth → `~/.jcode/auth.json` (and external imports);
   API key → `ANTHROPIC_API_KEY` env or `~/.config/jcode/<provider>.env`.
3. Ignore `auth-validation.json` verdicts older than 7 days (shown as `stale`);
   re-run `jcode auth-test` instead.

## Importing credentials from other agent tools

On a fresh install jcode can **reuse logins left behind by other coding
agents**, both OAuth tokens and API keys. Detection is consent-gated: jcode
lists the sources it found and only reads them after you approve each one
(`crates/jcode-base/src/auth/external.rs`, `unconsented_sources` /
`trust_external_auth_source`). Nothing is copied into jcode's own stores; the
external file is read in place.

Shared `auth.json`-style sources (`ExternalAuthSource`):

| Tool      | Auth file path                     | On-disk shape                                                                 |
|-----------|------------------------------------|------------------------------------------------------------------------------|
| OpenCode  | `~/.local/share/opencode/auth.json`| flat `{ provider: { type: "oauth", access, refresh, expires } \| { type: "api", key } }` |
| pi        | `~/.pi/agent/auth.json`            | flat `{ provider: { type: "oauth", ... } \| { type: "api_key", key } }` (key may be `$ENV` ref) |
| OpenClaw  | `~/.openclaw/agent/auth.json`, `~/.openclaw/agents/<id>/agent/auth-profiles.json`, `~/.openclaw/agents/<id>/agent/auth.json`, `~/.openclaw/credentials/oauth.json` | legacy flat pi shape, or the current `{ "profiles": { "<provider>:<name>": ... } }` store (first existing path wins; `main` agent and `:default` profiles preferred) |
| Hermes    | `~/.hermes/auth.json`              | nested `{ credential_pool: { provider: [ { auth_type, access_token, refresh_token, expires_at_ms } ] }, providers: {...} }` |

Notes:

- pi/OpenClaw API-key values that are `$ENV_VAR` references are resolved against
  the environment; values that begin with `!` (shell commands) are **never
  executed** and are skipped.
- Hermes stores literal API keys in the `access_token` field of `api_key`
  credential-pool entries; many of its providers store only env-var *names*, so
  those import nothing unless the env var is set.
- Other tool-specific importers exist for Claude Code, Codex, Gemini CLI,
  GitHub Copilot, and Cursor (see `auth/claude.rs`, `auth/codex.rs`,
  `auth/gemini.rs`, `auth/copilot.rs`, `auth/cursor.rs`).
