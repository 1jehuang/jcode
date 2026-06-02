# Configuration files reference

This document covers the **on-disk configuration files** jcode reads, with a
focus on **offline** / **air-gapped** / **self-hosted** setups (issue
[#48](https://github.com/quangdang46/jcode/issues/48)). All paths are
relative to your jcode home, which is `~/.jcode/` by default (or
`$JCODE_HOME` if set, or `$XDG_DATA_HOME/jcode` when
[`JCODE_USE_XDG=1`](#xdg-mode) is enabled).

## tl;dr

For a fully offline setup against a local OpenAI-compatible endpoint
(vLLM, llama.cpp, LM Studio, Ollama, etc.):

```toml
# ~/.jcode/config.toml
[provider]
default_provider = "local-vllm"
default_model    = "Qwen/Qwen3-Coder-30B-A3B-Instruct"

[providers.local-vllm]
type        = "openai-compatible"
base_url    = "http://localhost:8000/v1"
default_model = "Qwen/Qwen3-Coder-30B-A3B-Instruct"

[[providers.local-vllm.models]]
id            = "Qwen/Qwen3-Coder-30B-A3B-Instruct"
context_window = 128000
```

Plus run with `--offline` (or `JCODE_OFFLINE=1`) to disable the update
check + telemetry:

```bash
jcode --offline --provider-profile local-vllm
```

## Locations

| Path | Purpose | Format |
|---|---|---|
| `~/.jcode/config.toml` | Main config (providers, defaults, features) | TOML |
| `~/.jcode/auth.json` | Anthropic / Claude OAuth credentials | JSON |
| `~/.jcode/openai-auth.json` | OpenAI / Codex OAuth credentials | JSON |
| `~/.jcode/gemini_oauth.json` | Gemini OAuth credentials | JSON |
| `~/.jcode/mcp.json` | Global MCP server registry | JSON |
| `.jcode/mcp.json` (project) | Project-local MCP servers | JSON |
| `~/.jcode/prompts/*.md` | User-level prompt templates | Markdown |
| `.jcode/prompts/*.md` (project) | Project-level prompt templates | Markdown |
| `~/.jcode/SYSTEM.md` | Global system-prompt override | Markdown |
| `~/.jcode/APPEND_SYSTEM.md` | Global system-prompt append | Markdown |
| `.jcode/SYSTEM.md` (project) | Project system-prompt override | Markdown |
| `.jcode/APPEND_SYSTEM.md` (project) | Project system-prompt append | Markdown |
| `~/.jcode/sessions/` | Persisted session state (autosaved) | JSON per file |
| `~/.jcode/logs/jcode-YYYY-MM-DD.log` | Daily log output | text |
| `~/.config/jcode/<provider>.env` | Per-provider env-file overrides | dotenv |

> Project-level files always win over user-level files of the same name.

## `~/.jcode/config.toml` — main config

The main config has the following top-level tables (all optional):

```toml
[provider]
default_provider = "anthropic"     # provider key (oauth or compat profile)
default_model    = "claude-sonnet-4-5"

[providers.<name>]                 # OpenAI-compatible profile
type          = "openai-compatible"
base_url      = "https://...:port/v1"
default_model = "..."
api_key_env   = "MY_API_KEY"       # optional; reads env var
env_file      = "my-provider.env"  # optional; reads ~/.config/jcode/<file>
no_api_key    = false              # set true for local servers without auth

[[providers.<name>.models]]
id            = "model-id"
context_window = 128000

[features]
memory   = true   # enable embedding-based memory (requires local model)
swarm    = true   # multi-agent collaboration in same repo

[ambient]
enabled = false   # OpenClaw-style ambient mode
```

### Setting up an OpenAI-compatible provider via CLI

You usually don't need to edit this file by hand. The
[`jcode provider add`](../README.md#config-file-setup-for-self-hosted-endpoints-and-mcp)
command writes the profile for you, including secret-safe API key storage:

```bash
# Hosted OpenAI-compatible API (with API key from stdin):
printf '%s' "$MY_API_KEY" | jcode provider add my-api \
  --base-url https://llm.example.com/v1 \
  --model my-model-id \
  --api-key-stdin \
  --set-default

# Local server with no auth:
jcode provider add local-vllm \
  --base-url http://localhost:8000/v1 \
  --model Qwen/Qwen3-Coder-30B-A3B-Instruct \
  --no-api-key \
  --set-default
```

After adding, smoke-test it:

```bash
jcode --provider-profile local-vllm auth-test \
  --prompt 'Reply exactly JCODE_PROVIDER_SETUP_OK'
```

## `~/.jcode/mcp.json` — MCP server registry

```json
{
  "servers": {
    "filesystem": {
      "command": "/path/to/mcp-server",
      "args": ["--root", "/workspace"],
      "env": {},
      "shared": true
    }
  }
}
```

- Project-local equivalents live at `.jcode/mcp.json` (relative to cwd) and
  override entries with the same name.
- Compatibility fallback: if `~/.jcode/mcp.json` doesn't exist on first run,
  jcode imports from `~/.claude/mcp.json` and `~/.codex/config.toml`.
- Run [`jcode mcp trust <path>`](../docs/SAFE_EVALUATION.md) to mark a config
  as trusted when `--require-mcp-trust` is in effect.

## `~/.jcode/SYSTEM.md` and `APPEND_SYSTEM.md` — system-prompt overrides

- `SYSTEM.md` **replaces** jcode's built-in system prompt entirely.
- `APPEND_SYSTEM.md` **appends** to the built-in system prompt without
  removing it. Use this for "always remember to ..."-style additions.

Project-level `.jcode/SYSTEM.md` and `.jcode/APPEND_SYSTEM.md` (relative to
cwd) override the user-level versions.

## `~/.jcode/prompts/<name>.md` — slash-command templates

Discoverable via `jcode prompts list` and invokable as `/<name>` inside the
TUI. Scaffold one with:

```bash
jcode prompts new <name>           # → ./.jcode/prompts/<name>.md
jcode prompts new <name> --user    # → ~/.jcode/prompts/<name>.md
```

See [PR #207 + #217](https://github.com/quangdang46/jcode/pulls?q=prompts) for
the full template format.

## XDG mode

If you set `JCODE_USE_XDG=1`, the home moves to:

```
$XDG_DATA_HOME/jcode               (when XDG_DATA_HOME is set)
~/.local/share/jcode               (default fallback)
```

All file names above are unchanged — only the parent directory moves.
See [PR #225](https://github.com/quangdang46/jcode/pull/225) for the
toggle.

## Offline / air-gapped checklist

For machines that cannot reach the public internet:

1. **Disable the update check + telemetry**: `--offline` or
   `JCODE_OFFLINE=1`.
2. **Configure a local provider**: see the tl;dr above for a vLLM-style
   `config.toml`. Public OAuth flows (Claude, OpenAI, Gemini) won't work
   without internet.
3. **MCP servers**: install + register them in `~/.jcode/mcp.json` ahead of
   time. Project-local `.jcode/mcp.json` is fine for repo-pinned
   integrations.
4. **No memory embeddings**: the default memory backend uses local
   `tract-onnx` weights downloaded once. If your machine never had
   internet, copy `~/.jcode/embeddings/` from another machine, or set
   `[features] memory = false`.
5. **`jcode doctor`**: runs without network access; use it to verify the
   above before depending on jcode in production.

## See also

- [Z.AI Coding Plan quickstart](ZAI_CODING_PLAN.md)
- [Safe evaluation mode](SAFE_EVALUATION.md)
- [`jcode --help`](../README.md#further-reading) for the full CLI surface.
