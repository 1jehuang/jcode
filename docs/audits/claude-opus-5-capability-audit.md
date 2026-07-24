# Claude Opus 5 capability audit

Date: 2026-07-24 (Opus 5 general availability)
Method: live probes against `https://api.anthropic.com/v1/messages` with an
Anthropic API key, `anthropic-version: 2023-06-01`.

This exists because jcode's Claude capability tables are hand-maintained, and
the live catalog's `max_input_tokens` is known to over-advertise (see
`anthropic_context_mode`). Each row below is an observed API response, not a
docs claim.

## Observed behavior

| Capability | Probe | Result | jcode encoding |
|---|---|---|---|
| Model id | `GET /v1/models` | `claude-opus-5`, created 2026-07-24 | `ALL_CLAUDE_MODELS` |
| Max output | `max_tokens: 128000` | `200` | `anthropic_max_output_tokens` -> `128_000` |
| Max output ceiling | `max_tokens: 128001` | `400 "128001 > 128000, which is the maximum allowed number of output tokens for claude-opus-5"` | same |
| Context window | catalog `max_input_tokens` | `1000000` | `AnthropicContextMode::Native1M` |
| Adaptive thinking | `thinking: {type: adaptive}` | `200` | `model_supports_adaptive_thinking` |
| Manual thinking | `thinking: {type: enabled, budget_tokens}` | `400 "thinking.type.enabled is not supported for this model"` | not manual-thinking |
| Effort ladder | `output_config.effort` in `low/medium/high/xhigh/max` | all `200` | full modern ladder |
| Priority tier | `service_tier: auto` | `200`, response `usage.service_tier = standard` | not tier-eligible (same as Opus 4.8 on this account) |
| Pricing | Anthropic pricing page | `$5 / MTok` in, `$25 / MTok` out | `anthropic_api_pricing_with_tier` |

## Why the output ceiling mattered

jcode previously sent a flat `max_tokens = 32768` for every Claude model. Opus 5
allows 128K and uses always-on adaptive thinking, so its thinking plus the
visible tool call routinely exceeded 32K. Turns were truncated mid-tool-call and
agent runs ended early: the first Opus 5 benchmark cell exited cleanly after
using 4.2% of a 20-hour budget.

Fixed in `b9b1470ad` by deriving the budget per model. Opus 4.6-4.8, Sonnet
5/4.6, and Fable 5 share the 128K ceiling; Haiku 4.5 is 64K; unknown and older
ids keep the conservative 32K.

## Reproducing

```bash
set -a; source ~/.config/jcode/anthropic.env; set +a

# Output ceiling.
for mt in 128000 128001; do
  curl -s -o /dev/null -w "max_tokens=$mt http=%{http_code}\n" \
    https://api.anthropic.com/v1/messages \
    -H "x-api-key: $ANTHROPIC_API_KEY" -H "anthropic-version: 2023-06-01" \
    -H "content-type: application/json" \
    -d "{\"model\":\"claude-opus-5\",\"max_tokens\":$mt,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"
done

# Thinking modes.
curl -s https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-opus-5","max_tokens":4096,"thinking":{"type":"enabled","budget_tokens":2048},"messages":[{"role":"user","content":"2+2?"}]}' \
  | jq -r '.error.message'
```
