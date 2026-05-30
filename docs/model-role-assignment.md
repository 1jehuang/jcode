# Pareto-Optimal Model Assignment for jcode Specialized Agent Roles

| | |
|---|---|
| Status | Reference / recommended defaults |
| Last verified | 2026-05-30 |
| Method | Live provider catalogs queried via the wired OAuth credentials (not the hardcoded fallback lists) |
| Snapshot | `/tmp/jcode_models/live_catalog.json` (ephemeral; regenerate with the commands in section 6) |
| Code impact | `crates/jcode-base/src/sidecar.rs` (sidecar OAuth fallback), `crates/jcode-provider-core/src/models.rs` (`ALL_OPENAI_MODELS`) |

These are recommended values, not enforced defaults: each role's model is left
`None` in config so the runtime picks the provider's strongest model unless the
user overrides it. Section 4 lists the values to set when you want the
Pareto-optimal pick for a role.

## 1. Live model catalogs (verified via API)

### OpenAI / Codex backend
Endpoint: `https://chatgpt.com/backend-api/codex/models?client_version=1.0.0`
(auth: `~/.codex/auth.json` `tokens.access_token`).

| slug | ctx | reasoning levels | priority | notes |
|---|---|---|---|---|
| `gpt-5.5` | 272k | low/medium/high/xhigh | 9 | frontier coding model |
| `gpt-5.4` | 272k | low/medium/high/xhigh | 16 | strong generalist |
| `gpt-5.4-mini` | 272k | low/medium/high/xhigh | 23 | cheap, large ctx (NOT in hardcoded catalog) |
| `gpt-5.3-codex` | 272k | low/medium/high/xhigh | 25 | codex-tuned |
| `gpt-5.3-codex-spark` | 128k | low/medium/high/xhigh | 26 | fast, default reasoning=high |
| `gpt-5.2` | 272k | low/medium/high/xhigh | 29 | older generalist |
| `codex-auto-review` | 272k | low/medium/high/xhigh | 43 | hidden; vendor's dedicated review model |

### Antigravity / Gemini (cloudcode-pa)
Endpoint: `https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels`
(auth: agy account token at `~/.antigravity_tools/accounts/<id>.json`; the
Gemini-CLI token at `~/.gemini/oauth_creds.json` is `PERMISSION_DENIED` here).
Tier: **Google AI Ultra**.

| name | display | max_tok | thinking | vendor role hint |
|---|---|---|---|---|
| `gemini-3.1-pro-high` | Gemini 3.1 Pro (High) | 1.05M | yes | tiered:pro (deprecated -> `gemini-pro-agent`) |
| `gemini-pro-agent` | Gemini 3.1 Pro (High) | 1.05M | yes | agent-grade pro |
| `gemini-3.1-pro-low` | Gemini 3.1 Pro (Low) | 1.05M | yes | tiered:pro |
| `gemini-3-flash-agent` | Gemini 3.5 Flash (High) | 1.05M | yes | tiered:flash |
| `gemini-3.5-flash-low` | Gemini 3.5 Flash (Medium) | 1.05M | yes | **defaultAgentModelId** |
| `gemini-3.5-flash-extra-low` | Gemini 3.5 Flash (Low) | 1.05M | yes | cheap agent |
| `gemini-3-flash` | Gemini 3 Flash | 1.05M | yes | commandModelIds |
| `gemini-3.1-flash-lite` | Gemini 3.1 Flash Lite | 1.05M | no | tiered:flashLite, webSearch/mquery |
| `gpt-oss-120b-medium` | GPT-OSS 120B (Medium) | 131k | yes | OSS option |
| `claude-sonnet-4-6` | Claude Sonnet 4.6 (Thinking) | 250k | yes | via antigravity proxy |
| `claude-opus-4-6-thinking` | Claude Opus 4.6 (Thinking) | 250k | yes | via antigravity proxy |
| `gemini-2.5-pro` | Gemini 2.5 Pro | 1.05M | yes | legacy |
| (+ tab/image/lite variants) | | | | non-chat |

Vendor role hints from the same response:
`defaultAgentModelId=gemini-3.5-flash-low`,
`commandModelIds=[gemini-3-flash]`,
`webSearchModelIds/mqueryModelIds=[gemini-3.1-flash-lite]`,
`tieredModelIds={flashLite: gemini-3.1-flash-lite, flash: gemini-3-flash-agent, pro: gemini-3.1-pro-low}`.

### xAI / Grok
Endpoint: `https://api.x.ai/v1/language-models` (auth: `~/.grok/auth.json`
OIDC `key`). Profile in repo: `XAI_PROFILE` (`api.x.ai/v1`, default
`grok-code-fast-1`).

| id | in price | out price |
|---|---|---|
| `grok-4.3` | 12500 | 25000 |
| `grok-4.20-0309-reasoning` | 12500 | 25000 |
| `grok-4.20-0309-non-reasoning` | 12500 | 25000 |
| `grok-4.20-multi-agent-0309` | 12500 | 25000 |
| `grok-build-0.1` | 10000 | 20000 |

`grok-build-0.1` and `grok-4.20-multi-agent-0309` remain first-class (per
standing preference). Prices are micro-units per the xAI API; relative scaling
only.

## 2. jcode role -> config key mapping (verified)

| Role | Config key | Current default |
|---|---|---|
| Primary coding | `provider.default_model` + `provider.default_provider` | none (provider strongest) |
| Swarm subagents | `agents.swarm_model` | none (inherits) |
| Memory sidecar / side panel | `agents.memory_model`; `sidecar.rs` consts | OpenAI `gpt-5.3-codex-spark` -> fallback `gpt-5.4` -> Claude `claude-haiku-4-5` |
| Autoreview | `autoreview.model` | none |
| Autojudge | `autojudge.model` | none |
| Ambient / orchestrator | `ambient.model` + `ambient.provider` | none (provider strongest) |

There is no separate "side panel model" role; the side panel is driven by the
memory sidecar.

## 3. Pareto reasoning

Each role is scored on capability (benchmark/agentic strength), latency
(time-to-first-token + throughput), and cost (token price / quota burn). A model
is Pareto-optimal for a role when no other available model is at least as good on
all three axes and strictly better on one, for that role's workload.

Role workload profiles:
- Primary coding: high capability dominant, latency secondary, cost tertiary.
- Swarm subagents: parallel fan-out, so cost + latency dominate; capability
  "good enough" since work is decomposed.
- Memory sidecar: very high frequency, tiny tasks (relevance/extraction);
  latency + cost dominate, capability minimal.
- Autoreview: capability dominant (catching real bugs), latency irrelevant
  (end-of-turn), cost secondary.
- Autojudge: structured verdicts; mid capability, low latency, low cost.
- Ambient: long-horizon autonomous; capability dominant, cost matters (runs
  unattended), latency irrelevant.

## 4. Assignments

| Role | Primary (OpenAI-first) | Antigravity alt | Grok alt | Rationale |
|---|---|---|---|---|
| Primary coding | `gpt-5.5` (high) | `gemini-3.1-pro-high` | `grok-4.3` | Frontier coding; top priority slug 9. 272k ctx. |
| Swarm subagents | `gpt-5.4-mini` | `gemini-3.5-flash-low` (vendor default agent) | `grok-build-0.1` | Cheapest capable agent tier; large ctx; built for fan-out. |
| Memory sidecar | `gpt-5.3-codex-spark` (keep) -> `gpt-5.4-mini` | `gemini-3.1-flash-lite` | `grok-build-0.1` | High-frequency tiny tasks; spark is fast. flash-lite is vendor's mquery/search pick. |
| Autoreview | `gpt-5.3-codex` | `gemini-pro-agent` | `grok-4.20-0309-reasoning` | Codex-tuned for code review; `codex-auto-review` is hidden so use codex slug. |
| Autojudge | `gpt-5.4` | `gemini-3-flash-agent` | `grok-4.20-0309-reasoning` | Structured verdicts; balanced capability/latency. |
| Ambient/orchestrator | `gpt-5.5` (medium) | `gemini-3.1-pro-high` | `grok-4.20-multi-agent-0309` | Long-horizon autonomy; multi-agent grok is purpose-built. |

Notes:
- Sidecar already prefers `gpt-5.3-codex-spark`; keep but add `gpt-5.4-mini` as a
  cheaper/larger-ctx alternative now that it is live (it was missing from the
  hardcoded catalog). This is now applied in `sidecar.rs`.
- `codex-auto-review` exists but has `visibility=hide`; do not surface it in the
  picker. Use `gpt-5.3-codex` for the autoreview role instead.
- For Grok, autoreview/autojudge should use a reasoning variant
  (`grok-4.20-0309-reasoning`), not the non-reasoning one.

### Config example (OpenAI-first picks)

Set these in the jcode config to pin the Pareto picks per role:

```toml
[provider]
default_provider = "openai"
default_model = "gpt-5.5"

[agents]
swarm_model = "gpt-5.4-mini"
memory_model = "gpt-5.3-codex-spark"

[autoreview]
model = "gpt-5.3-codex"

[autojudge]
model = "gpt-5.4"

[ambient]
provider = "openai"
model = "gpt-5.5"
```

## 5. Catalog drift to fix in code

The hardcoded fallback catalogs are stale relative to live:
- `crates/jcode-provider-core/src/models.rs` `ALL_OPENAI_MODELS` was missing
  `gpt-5.4-mini` (now added).
- `crates/jcode-provider-gemini/src/lib.rs` `AVAILABLE_MODELS` lists
  `gemini-3.1-pro-preview` / `gemini-3-pro-preview` / `gemini-3-flash-preview`,
  but the live Ultra-tier Antigravity catalog exposes `gemini-3.1-pro-high`,
  `gemini-pro-agent`, `gemini-3.5-flash-low`, `gemini-3-flash`,
  `gemini-3.1-flash-lite`, etc.

Recommend wiring the role defaults to read from the live catalog (already
fetched by `fetch_openai_model_catalog` / `fetchAvailableModels`) and only fall
back to the static lists when offline.

## 6. Reproducing the live catalog

The snapshot in the header is ephemeral. Regenerate it from the wired creds:

```bash
# OpenAI / Codex backend
CODEX_TOKEN=$(python3 -c "import json;print(json.load(open('$HOME/.codex/auth.json'))['tokens']['access_token'])")
curl -s "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0" \
  -H "Authorization: Bearer $CODEX_TOKEN"

# Antigravity / Gemini (uses the agy account token, NOT ~/.gemini)
ACC=$HOME/.antigravity_tools/accounts/$(python3 -c "import json;print(json.load(open('$HOME/.antigravity_tools/accounts.json'))['current_account_id'])").json
ATOKEN=$(python3 -c "import json;print(json.load(open('$ACC'))['token']['access_token'])")
APROJ=$(python3 -c "import json;print(json.load(open('$ACC'))['token']['project_id'])")
curl -s -X POST "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels" \
  -H "Authorization: Bearer $ATOKEN" -H "Content-Type: application/json" \
  -H "User-Agent: antigravity/1.18.3 darwin/arm64" \
  -H "x-goog-api-client: google-cloud-sdk vscode_cloudshelleditor/0.1" \
  -H 'client-metadata: {"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}' \
  -d "{\"project\":\"$APROJ\"}"

# xAI / Grok
GKEY=$(python3 -c "import json;d=json.load(open('$HOME/.grok/auth.json'));print(list(d.values())[0]['key'])")
curl -s "https://api.x.ai/v1/language-models" -H "Authorization: Bearer $GKEY"
```

Notes:
- The `~/.gemini/oauth_creds.json` token is `PERMISSION_DENIED` on
  `fetchAvailableModels`; that endpoint is gated to the Antigravity OAuth client,
  so the agy account token must be used.
- Tokens expire (Codex/Gemini ~1h, Grok ~6h); refresh via the respective CLI if
  a request returns 401/403 with an auth error.

