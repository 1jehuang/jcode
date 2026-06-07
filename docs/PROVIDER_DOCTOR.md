# Provider Doctor

`jcode provider-doctor` is a user-facing diagnostic that answers one question:

> Why isn't my provider/model (or the model picker) working?

It walks the same strict end-to-end checkpoints that the live coverage ledger
tracks (`jcode provider-test-coverage`), but as an interactive command you can run
yourself, with clear pass/fail output and a "what to try next" hint on the first
failure.

It works with **OpenAI-compatible providers** (cerebras, fpt, nvidia-nim,
comtegra, deepseek, groq, openrouter, and other `openai-compatible` profiles).

## Quick start

```bash
# Validate jcode's own wiring for a provider, no API key, no spend:
jcode provider-doctor cerebras --tier offline

# Validate the key + live model catalog (needs a key, negligible spend):
jcode provider-doctor cerebras --tier catalog

# Full readiness, including real chat, streaming, and tool calls (spends balance):
jcode provider-doctor cerebras --tier full

# Pin a specific model and emit JSON for scripting/CI:
jcode provider-doctor cerebras --model gpt-oss-120b --tier full --json
```

The model defaults to the provider's default model (or the first live catalog
model). Use the global `--model` flag to pin a specific one.

## Tiers

Pick how much to exercise. Each tier validates as much as is possible given its
constraints, so you can debug cheaply and escalate only when needed.

| Tier | Needs key? | Spends balance? | What it adds | Catches |
| --- | --- | --- | --- | --- |
| `offline` | no | no | jcode-side wiring against a synthetic catalog | catalog reload, picker rendering, fallback labeling, and model-switch routing bugs for this provider |
| `catalog` (default) | yes | ~none | live `GET /models` | bad/missing key, dead endpoint, model not in the live catalog |
| `full` | yes | yes | non-streaming chat, streaming, tool-call loop | the model actually chats, streams, and supports tool-calling |

Only the `full` tier can earn strict ("READY") coverage. The lighter tiers
intentionally record the API-dependent checkpoints as skipped, so nothing is
over-credited in the coverage ledger.

## Checkpoints

Every run reports these strict checkpoints in order. A pair is fully ready only
when all of them pass on the `full` tier.

1. `auth_credential_loaded` - a credential was found for the provider
2. `model_catalog_live_endpoint` - the live `/models` endpoint returned models
3. `catalog_hot_reload_current_session` - the catalog reloaded into the session
4. `picker_live_models` - the picker shows the live models, including the selected one
5. `picker_fallback_labeling` - routes are live-catalog backed, not static fallback
6. `model_switch_route` - switching models produces a provider-explicit route
7. `non_streaming_chat_completion` - a basic chat reply came back (full tier)
8. `streaming_chat_completion` - a streamed reply came back (full tier)
9. `tool_call_parse` - the model emitted a parseable tool call (full tier)
10. `tool_execution_loop` - the tool-call loop ran (full tier)
11. `tool_result_followup` - the tool result was fed back (full tier)
12. `real_jcode_tool_smoke` - an end-to-end tool smoke passed (full tier)

(Checkpoints 1-2 plus the auth-lifecycle stages are pre-flight; 7-12 are the
API-dependent ones gated behind `--tier full`.)

## Reading the output

```
Provider doctor: Cerebras / gpt-oss-120b
Tier: catalog (API key, ~no spend: adds live catalog fetch)
...
  [ PASS] Credential loaded                      Loaded credential from CEREBRAS_API_KEY
  [ PASS] Live model catalog endpoint            2 live model(s) returned
  [ PASS] Catalog hot reload in current session  2 catalog route(s) reloaded
  [ PASS] Picker shows live models               2 model(s) in picker, selected `gpt-oss-120b`
  [ PASS] Picker fallback labeling               all routes backed by live catalog (no static fallback)
  [ PASS] Model switch route                     switch request `cerebras:...` routed via `openai-compatible:cerebras`
  [ skip] Non-streaming chat completion          catalog tier: requires --tier full (spends balance)
  ...
Verdict: tier `catalog` passed. Run `--tier full` to confirm full readiness (spends balance).
```

- `PASS` / `FAIL` - the checkpoint ran and passed/failed.
- `skip` - the current tier does not run this checkpoint (use `--tier full`).
- The verdict line tells you whether the tier passed, fully passed (`READY`), or
  failed, and on failure points at the first failing checkpoint with a next step.

The command exits non-zero when the chosen tier did not fully pass, so it can be
used as a CI/scripting gate.

## Spend tracking (how much does a run cost?)

Balance-spending tiers (`catalog` makes a catalog call, `full` makes several
chat/stream/tool calls) report exactly what they consumed so you can budget:

```
Spend this run: 3 billable API calls, 554 tokens (289 in + 265 out), cost not reported by provider
```

- **billable API calls** - how many requests actually hit the provider.
- **tokens** - prompt + completion totals summed across those calls, when the
  provider returns a `usage` block. Streaming probes request
  `stream_options.include_usage` so streamed calls are counted too.
- **cost** - shown as a USD figure only when the provider reports a `cost`
  field; many providers (e.g. cerebras) only return tokens, so you'll see
  "cost not reported by provider" and can multiply tokens by your plan's rate.
  A full cerebras run is roughly 550-620 tokens (about $0.0003).

`--json` includes the same data under a `spend` object
(`billable_calls`, `prompt_tokens`, `completion_tokens`, `total_tokens`,
`has_token_data`, `reported_cost_usd`).

This spend is **persisted** into the coverage ledger alongside the run, so
`jcode provider-test-coverage` shows a cumulative "Recorded spend" footer
summing the latest run per pair. That gives you a durable, at-a-glance answer to
"how much has exercising this coverage cost me so far?"

## Typical debugging flow

1. **"My picker is broken / shows the wrong models."**
   Run `--tier offline`. If `picker_live_models`, `picker_fallback_labeling`, or
   `model_switch_route` fail, it's a jcode-side routing bug for that provider:
   capture the output and file an issue.

2. **"It won't connect / says auth failed."**
   Run `--tier catalog`. If `auth_credential_loaded` or
   `model_catalog_live_endpoint` fail, the key/endpoint is the problem. Run
   `jcode login --provider <provider>`.

3. **"It connects but the model behaves badly."**
   Run `--tier full`. If `non_streaming_chat_completion` /
   `streaming_chat_completion` / the `tool_*` checkpoints fail, the model itself
   is the issue; try another model from the live catalog.

## Relationship to coverage

Every doctor run records a live-verification event into the coverage ledger,
tagged with the tier (`doctor_tier`). A `full`-tier pass that clears all 11
strict checkpoints flips the pair to strict ("READY") in
`jcode provider-test-coverage`. Lighter tiers record the API-dependent
checkpoints as skipped, so they never over-credit a pair.

`jcode provider-test-coverage` renders the same 11 checkpoints as an 11-stage
pipeline. Each observed pair gets one compact line: a status token (`READY`, or
`N/11` = how many stages it cleared) followed by `provider / model`, and then,
for any pair that is not yet READY, the first blocker plus the exact
`provider-doctor` command to push it past that blocker. So the two commands are
two views of one pipeline: the coverage report shows where every pair is stuck
and hands you the doctor command to advance it.

Each line ends with a freshness note, e.g.:

```
  READY  cerebras / gpt-oss-120b   last tested 9 minutes ago (2026-05-30) by developer (dev build)
  6/11   nvidia-nim / gemma-4-31b  failed at `streaming reply`; run `jcode provider-doctor nvidia-nim --model gemma-4-31b --tier full`; last tested 2 days ago ...
```

- **how long ago** the most recent run was, in plain English plus the absolute
  date, so you can tell at a glance whether the evidence is stale.
- **who ran it**: a clean release build is labeled `user (release build)` (real
  user evidence), a dirty/dev build is `developer (dev build)`. This is derived
  durably from the build flag recorded with each run, not guessed.
