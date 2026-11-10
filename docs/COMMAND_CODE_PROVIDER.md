# Command Code Provider

JCode ships Command Code as a built-in OpenAI-compatible profile with a
Command Code-specific runtime path. Users select it as `command-code`, log in
with a Command Code API key, and do not need to configure a generic
OpenAI-compatible provider.

## Provider contract

| Item | Value |
| --- | --- |
| Provider id | `command-code` |
| Display name | Command Code |
| API base | `https://api.commandcode.ai/provider/v1` |
| API key env | `COMMAND_CODE_API_KEY` |
| JCode env file | `command-code.env` |
| Setup URL | <https://commandcode.ai/docs/provider> |
| Default model | `deepseek/deepseek-v4-flash` |

The API key is a long-lived bearer credential. Command Code does not document a
public OAuth refresh flow for third-party clients. JCode does not invent one.

## Endpoints

Command Code's public Provider API exposes:

- `POST /chat/completions` for OpenAI Chat Completions compatible models.
- `POST /responses` for OpenAI Responses compatible models.
- `POST /messages` for Anthropic Messages compatible models.
- `GET /models` for the live model catalog.

The catalog's `supported_endpoints` field is authoritative. JCode must never
send a model to an endpoint the catalog did not advertise.

The internal `/alpha/generate` endpoint used by the Command Code CLI is not a
public third-party contract and is not used by JCode.

## Transport policy

| Catalog endpoints | Transport |
| --- | --- |
| `/messages` only | Anthropic Messages |
| `/chat/completions` only | Chat Completions |
| `/chat/completions` and `/responses` | Chat Completions, except Muse models |
| `/responses` only | Responses |

Muse models use Responses because Command Code accepts `prompt_cache_key` there
and returns encrypted reasoning content with
`include: ["reasoning.encrypted_content"]`. Other models remain on Chat until
their Responses tool and reasoning behavior is verified per family.

The provider keeps its stable JCode identity even though it reuses the existing
compatible-profile runtime internally. It is not displayed as OpenRouter and is
not a manually configured OpenAI-compatible profile.

## Authentication

Baseline login is an API key created in Command Code Studio:

```sh
jcode login --provider command-code
```

The key is stored in JCode's Command Code account store
(`command_code_accounts.json`, mode `0600`). The store supports labeled
accounts:

```sh
jcode login --provider command-code --account work
jcode login --provider command-code --account personal
```

An unnamed login receives an auto-generated label. Logging in again with the
same label replaces that account's key and makes it active. To switch to an
already-stored account without pasting the key again:

```sh
jcode login --provider command-code --account work
```

When the label exists and no new key is supplied, JCode selects it and validates
the stored credential. The existing `COMMAND_CODE_API_KEY` environment variable
still takes precedence over the active stored account. A legacy
`command-code.env` file remains a fallback for older installations.

Post-login validation uses the authenticated `/alpha/whoami` endpoint as a best
effort check. That endpoint is used by Command Code's own CLI but is not
documented as a third-party contract. An explicit 401 or 403 fails the login. A
network failure or unavailable validator prints a warning and leaves the saved
key for the first model request to confirm.

`GET /provider/v1/models` is public and returns HTTP 200 even for an invalid
key. It is never used as a key-validity check.

## Models and capabilities

The live catalog is the authoritative model list. The bundled static list is an
offline fallback, not a release allowlist. A successful refresh replaces it. A
failed refresh preserves the last good catalog.

Observed catalog shape on 2026-11-10:

- 97 models.
- 12 models advertise only `/messages`.
- 72 advertise `/chat/completions` and `/responses`.
- 13 advertise only `/chat/completions`.

Context windows come from the catalog `context_length`. JCode does not invent a
context window that the catalog did not report. Model ids are case-sensitive and
may contain `/` and `:`.

An explicitly selected model is never silently replaced. `MODEL_NOT_IN_PLAN`
and `unsupported_model` are surfaced as errors. When the catalog is unavailable,
an explicit model id is accepted and the provider request is the authority.

## Tools

Command Code accepts standard client-executed tools on the supported endpoints.
JCode retains its own tool loop, system prompt, workspace, and persistence.

- Chat: standard OpenAI `tools`, `tool_calls`, and tool-role results.
- Responses: standard `function` tools, `function_call` items, and
  `function_call_output` items.
- Messages: standard Anthropic `tool_use` and `tool_result` blocks.

The provider emits normal `ToolUseStart`, `ToolInputDelta`, and `ToolUseEnd`
events. It does not emit `NativeToolCall`, so tool results cannot be silently
dropped by a missing native-result channel.

## Streaming

Every transport requires a real terminal signal:

- Chat: `[DONE]` or a non-empty finish reason.
- Responses: `response.completed` or `response.incomplete`.
- Messages: `message_stop` or the Anthropic runtime's completed message delta.

An HTTP body that ends before a terminal signal is an incomplete-stream error,
not a successful completion. Malformed SSE frames and provider error events are
surfaced as errors.

Idle timeouts and bounded retries use JCode's shared provider stream and retry
machinery. Command Code does not add a second cooldown subsystem.

## Prompt caching

For Command Code, JCode sends an opaque `prompt_cache_key` derived from the
provider instance's stable conversation id. It contains no transcript content,
credential, email, or workspace path. A new provider fork gets a new key.

Observed on 2026-11-10 with `meta/muse-ember` over Responses:

- Five-turn keyed chain cache reads: 1012, 1140, 1140, 1140 input tokens.
- Comparable unkeyed chain: 1012, 0, 1140, 1140.

A live JCode Muse tool turn reported `upload: 15691`, `cache_read: 15476`
(98.6 percent of the second request). A resumed Chat session reported
`upload: 14130`, `cache_read: 14080`.

These are observations, not a guarantee. JCode reports cache reads only when
the provider returns them. Missing cache values remain unknown, never zero.

## Usage and quota

Per-response token usage is mapped for all transports:

- Chat: `prompt_tokens`, `completion_tokens`, cache-read details.
- Responses: `input_tokens`, `output_tokens`, `input_tokens_details`.
- Messages: `message_start` and `message_delta` usage.

Command Code documents Studio usage and the CLI `/usage` command, but no public
account-quota API. JCode therefore reports:

```
Key status: configured (no documented key-status endpoint)
Quota: unknown (no documented account-quota API)
```

JCode does not invent remaining credits, percentages, reset times, or billing
data. The CLI's `/alpha/billing/credits` endpoint is unofficial and is not on
the generation critical path.

## Error handling

| Class | Behavior |
| --- | --- |
| 401 unauthorized | No retry, clear re-login message |
| 403 model not in plan | No retry, no model substitution |
| 402 or insufficient credits | No retry, clear quota message |
| 429 rate limit | Bounded retry with backoff and `Retry-After` when present |
| 5xx or transient network | Bounded retry before output |
| 400 unsupported model | No retry, transport-selection error |
| Malformed or incomplete stream | Error, never a successful completion |

## Known limitations

- Accounts are selected at login time. The provider resolves the active account
  when it builds its request authentication. JCode does not perform automatic
  cross-account failover when the active account is rate-limited or out of plan.
- Browser-assisted login is not implemented. The API key flow is official and
  is the supported path.
- Claude models are routed through Anthropic Messages, but the test account used
  for this change had no Claude entitlement. The transport and authorization
  path were verified up to the provider's `MODEL_NOT_IN_PLAN` response, but a
  successful Claude generation was not executed here.
- The public catalog does not include plan entitlements. A model may appear in
  the catalog and still return `MODEL_NOT_IN_PLAN`.
- Command Code does not publish a stable third-party quota API.

## Test evidence

See `COMMAND_CODE_IMPLEMENTATION_STATUS.md` for exact commands, commits, and
pass/fail results. Live tests in this document were run with isolated
`JCODE_HOME` and socket paths. No production JCode state was modified.
