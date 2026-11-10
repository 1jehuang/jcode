# Command Code Implementation Status

Last updated: 2026-11-10

## Repository state

- Upstream: `https://github.com/1jehuang/jcode`
- Upstream base SHA examined: `e43279cdd`
- Feature branch: `command-code-native-provider`
- Working tree: clean after each milestone commit
- No upstream branch was modified, forced-pushed, or merged.
- Local git identity is `ddawnlll <ddawnlll@users.noreply.github.com>` because no
  global git identity was configured on this machine.

## Decision log

### D1. Use the existing compatible-profile runtime instead of a new provider slot

- Original plan: install Command Code as a built-in OpenAI-compatible profile,
  with a dedicated runtime if necessary.
- Evidence: `MultiProvider` already has a first-class compatible-profile map,
  route builder, model picker, auth status, session restore, and factory hook.
  A new `ActiveProvider` variant would require touching dozens of match arms and
  a large set of integration paths.
- Decision: keep the `command-code` profile identity and extend the existing
  compatible runtime with Command Code-specific request, transport, and cache
  branches. No new `ActiveProvider` variant.
- Effect: smaller integration surface, no duplicate route/auth framework.

### D2. Do not implement `/alpha/generate`

- Original plan: use only documented Provider API endpoints.
- Evidence: live probes confirmed `/chat/completions`, `/responses`, and
  `/messages` are documented and functional. `/alpha/generate` is the CLI's
  internal NDJSON endpoint and is not a public provider contract.
- Decision: production code does not call `/alpha/generate`.
- Effect: removes PR #1271's largest upstream blocker.

### D3. Reuse the Anthropic runtime through an explicit constructor

- Original plan: add a Messages transport while reusing established Anthropic
  serialization and streaming.
- Evidence: the Anthropic runtime already has complete Messages request
  serialization, tool handling, cache control, usage accounting, and SSE parsing.
  Duplicating that parser would be large and risky.
- Decision: add `AnthropicProvider::new_command_code(base, key, model)` in a new
  `compatible.rs` module. The constructor pins bearer auth, the Messages URL,
  API-key mode, and the model list without process-global env mutation.
- Effect: Claude models reuse the mature Anthropic stream and tool loop.

### D4. Use Responses for Muse only

- Original plan: route by catalog `supported_endpoints`, prefer Responses where
  it improves cache and reasoning replay.
- Evidence: live tests confirmed Muse Responses accepts `prompt_cache_key` and
  returns encrypted reasoning with `include`. Other Responses-capable models
  remain unverified for tool and reasoning replay.
- Decision: Muse models use Responses. Other dual-endpoint models use Chat until
  verified per family.
- Effect: high Muse cache reuse without broad unverified transport changes.

### D5. Keep quota honest

- Original plan: never fabricate quota metrics.
- Evidence: Command Code publishes Studio usage and CLI `/usage`, but no public
  quota API. `/alpha/billing/credits` is unofficial.
- Decision: report per-response token usage and show quota as unknown. Do not
  call billing on the generation path.
- Effect: `/usage` is honest, and generation cannot stall on a billing endpoint.

## Milestones

### M1: Metadata, catalog, and size-budget compliance

Status: complete.

Commit: `67b2f3b58` (`feat(provider): add Command Code metadata and endpoint catalog fields`).

Implemented:

- `COMMAND_CODE_PROFILE`.
- `COMMAND_CODE_LOGIN_PROVIDER`.
- Dated `COMMAND_CODE_STATIC_MODELS` fallback.
- `supported_endpoints` on `ModelInfo` and in the catalog parser.
- Command Code static models and context limits.
- Moved profile/login registry arrays to `registry.rs` to create code-size
  headroom.

Verification:

- `cargo test -p jcode-provider-metadata`: 21 passed, 0 failed.
- `cargo test -p jcode-provider-openrouter-runtime --lib models_catalog_parse`:
  13 passed, 0 failed, including the new Command Code endpoint fixture.
- `cargo check -p jcode-base --lib`: passed.

Notes:

- `scripts/check_code_size_budget.py` is already red on upstream master because
  its baseline predates many unrelated master changes. No Command Code file
  regressed; the new code was placed in new files and the touched oversized
  `catalog.rs` and `provider_catalog.rs` shrank.

### M2: Native login and usable Chat transport

Status: complete.

Commit: `1150c2a56` (`feat(provider): wire Command Code native login and Chat transport`).

Implemented:

- `ProviderChoice::CommandCode` and CLI `-p command-code`.
- Provider list entry.
- Generic compatible-profile API-key login.
- Post-login `/alpha/whoami` validation with explicit 401/403 failure and
  best-effort fallback when the validator is unavailable.
- Command Code `/usage` report with honest key and quota labels.
- Preserved the explicit Command Code default model instead of auto-promoting
  the newest catalog entry, which was a Messages-only Claude model.
- Temporary hiding of Messages-only models before M4.

Verification:

- `jcode provider list` includes `command-code`.
- Isolated `jcode login command-code --api-key ... --no-browser`:
  - credential saved to the isolated `command-code.env` with mode 0600,
  - whoami authenticated,
  - provider smoke returned `AUTH_TEST_OK`,
  - tool smoke completed a real JCode bash tool call and follow-up.
- Isolated real run:
  `jcode run -p command-code -m deepseek/deepseek-v4-flash "Use the bash tool to run exactly: echo alpha; echo beta. Then reply DONE."`
  - bash executed once,
  - persisted result contained `alpha\nbeta`,
  - second request included the result,
  - final answer was `DONE`,
  - session provider was `command-code` and model was
    `deepseek/deepseek-v4-flash`.
- `cargo test -p jcode --lib list_cli_providers_includes_command_code`: 1
  passed.

### M3: Responses transport, strict terminal handling, Muse cache

Status: complete for the implemented Muse path.

Commit: `40ac26a99` (`feat(provider): add Command Code Responses transport and strict stream terminals`).

Implemented:

- Command Code Muse models route over `/responses`.
- Responses request uses the native OpenAI Responses input builder, encrypted
  reasoning inclusion, tools, and `prompt_cache_key`.
- Command Code Responses uses the native `OpenAIResponsesStream` parser.
- Chat stream strict terminal mode: EOF without `[DONE]` or a finish reason is
  an error.
- Responses stream checks `response.completed` or `response.incomplete` and
  rejects a cut-off body.
- Tests for the Responses request fields, Muse routing, strict chat EOF, and
  strict chat `[DONE]`.

Verification:

- `cargo test -p jcode-provider-openrouter-runtime --lib command_code_responses`:
  2 passed.
- `cargo test -p jcode-provider-openrouter --lib strict_stream`: 2 passed.
- Isolated live Muse run:
  `jcode run -p command-code -m meta/muse-ember "..."`
  - logs show `format=responses`,
  - tool result contained `muse-alpha\nmuse-beta`,
  - final answer `DONE`,
  - token report `upload: 15691`, `cache_read: 15476` (98.6 percent).
- Strict chat mode regression: a normal DeepSeek Chat tool turn still completed
  and reported `cache_read: 13952` on the follow-up.

### M4: Anthropic Messages routing

Status: implemented and transport-verified. Successful Claude generation is
blocked by the available account's plan.

Commit: `b164330b7` (`feat(provider): route Command Code Claude models through Messages`).

Implemented:

- `AnthropicProvider::new_command_code` in
  `crates/jcode-provider-anthropic-runtime/src/compatible.rs`.
- Explicit bearer auth, `{base}/messages`, API-key mode, model list, and 32K
  output budget.
- Command Code Claude ids route through the Anthropic delegate.
- Removed the temporary Messages-only model filter, so all 97 catalog models are
  discoverable.

Verification:

- `cargo test -p jcode-provider-anthropic-runtime --lib command_code_constructor`:
  1 passed.
- Isolated live Claude attempt:
  `jcode run -p command-code -m claude-sonnet-5 "Reply with exactly OK"`
  - logs show `format=anthropic_messages`,
  - provider is `Command Code`,
  - the API returned 403
    `MODEL_NOT_IN_PLAN: Claude Sonnet 5 available in Pro and above plans...`.
  - This proves the Messages endpoint and auth path, but not a successful Claude
    turn.

### M5: Reliability and lifecycle hardening

Status: partial. Implemented and tested items listed below; cancellation and
full mock-server matrix are not yet implemented.

Commits: `53e851def`, `7ad660ec9`.

Implemented:

- Strict Chat terminal validation.
- Strict Responses terminal validation.
- Incomplete Responses parser test.
- Existing shared stream idle timeout and bounded retry machinery retained.
- Provider error events surfaced, not converted to success.

Verified:

- `cargo test -p jcode-provider-openrouter --lib strict_stream`: 2 passed.
- `cargo test -p jcode-provider-openai --lib incomplete_responses_stream`: 1
  passed.
- Session resume:
  `jcode run --resume session_shrimp_... -p command-code -m deepseek/deepseek-v4-flash`
  returned `RESUMED_OK` with `cache_read: 14080`.

Not yet verified:

- Explicit cancellation stress test.
- Mock-server 429, 5xx, stalled body, and split-frame matrix.
- Concurrent session isolation test.

### M6: Usage and quota honesty

Status: complete for the implemented surface.

Implemented:

- Command Code `/usage` arm.
- Public `/models` is not treated as a key validator.
- Quota reported as unknown because no public quota API exists.
- Per-response token and cache usage through existing JCode accounting.

Verified:

- `jcode usage -p command-code` in the isolated home prints:
  - `Key status: configured (no documented key-status endpoint)`
  - `Quota: unknown (no documented account-quota API)`
- Live Chat, Responses, and resumed turns reported input, output, and cache
  reads.

### M7: Multi-account lifecycle

Status: not implemented.

Reason: JCode's generic API-key compatible profile is single-key. A correct
multi-account implementation needs a shared account store, active-account
selection, TUI/CLI switching, per-account cache scope, and safe pre-stream
failover. Implementing only a Command Code-specific parallel store would violate
the plan's reuse requirement.

Remaining work:

- Extend or reuse `jcode-base` account storage for API-key profiles.
- Add explicit account switch.
- Add per-account cache key scope.
- Add bounded, pre-stream-only failover policy.
- Add account lifecycle tests.

### M8: Browser-assisted login

Status: not implemented.

Reason: Command Code's public docs describe browser login for its own CLI, but
the callback transport is not documented as a stable third-party contract. The
API-key flow is official and works. JCode does not fabricate OAuth or claim
browser support without a verifiable contract.

Remaining work:

- Confirm with Command Code that the loopback callback is a supported external
  application contract.
- Implement CORS preflight, split-read, dynamic loopback port, state validation,
  denial handling, and body limits if approved.

### M9: Release readiness

Status: in progress.

Implemented:

- `docs/COMMAND_CODE_PROVIDER.md`.
- This status file.
- Unit and live evidence for M1 through M6.

Remaining:

- Full mock-server matrix.
- Provider Doctor integration.
- Concurrency and cancellation tests.
- Upstream PR preparation and coordination with PR #1271.

## Exact commands and outcomes

### Unit and parser tests

| Command | Result |
| --- | --- |
| `cargo test -p jcode-provider-metadata` | 21 passed, 0 failed |
| `cargo test -p jcode-provider-openrouter-runtime --lib models_catalog_parse` | 13 passed, 0 failed |
| `cargo test -p jcode-provider-openrouter-runtime --lib command_code_responses` | 2 passed, 0 failed |
| `cargo test -p jcode-provider-openrouter --lib strict_stream` | 2 passed, 0 failed |
| `cargo test -p jcode-provider-openai --lib incomplete_responses_stream` | 1 passed, 0 failed |
| `cargo test -p jcode-provider-anthropic-runtime --lib command_code_constructor` | 1 passed, 0 failed |
| `cargo test -p jcode --lib list_cli_providers_includes_command_code` | 1 passed, 0 failed |

### Compile and build

| Command | Result |
| --- | --- |
| `cargo check -p jcode-base --lib` | passed |
| `cargo check -p jcode --bin jcode` | passed |
| `cargo build -p jcode --bin jcode` | passed |

### Live, isolated

All live tests used `JCODE_HOME=/home/quant/src/jcode-commandcode-fixes/tmp-cc-home2`
and a dedicated socket under that directory. The production JCode config,
credentials, and shared daemon were not modified.

| Check | Result |
| --- | --- |
| `jcode login command-code --api-key ... --no-browser` | PASS, whoami verified, provider smoke `AUTH_TEST_OK`, tool smoke `AUTH_TEST_OK` |
| Chat tool round trip on `deepseek/deepseek-v4-flash` | PASS, tool result `alpha\nbeta`, final `DONE` |
| Responses tool round trip on `meta/muse-ember` | PASS, `format=responses`, cache_read 15476 |
| Session resume | PASS, `RESUMED_OK`, cache_read 14080 |
| `/usage` honesty | PASS, configured key and unknown quota |
| Claude Messages path | BLOCKED by account plan, 403 `MODEL_NOT_IN_PLAN`, endpoint and auth verified |

## Current blockers

1. Claude generation cannot be verified on the available account because it has
   no Claude entitlement. A Pro, Provider, or other Claude-entitled account is
   required for the final M4 acceptance test.
2. Multi-account requires a generic JCode API-key account store and is not
   implemented in this change.
3. Browser login requires confirmation that the Command Code CLI callback is a
   supported third-party contract.
4. The upstream code-size baseline is stale on master. The Command Code changes
   do not regress any tracked file, but `scripts/check_code_size_budget.py`
   cannot be used as a green gate without a separate baseline cleanup.
5. Full mock-server and concurrency coverage is not yet implemented.

## Next independently verifiable task

Implement a local mock-server integration suite for the Command Code runtime:

- Chat SSE with tool call and follow-up.
- Responses SSE with function call and follow-up.
- Messages SSE with tool call and follow-up.
- Incomplete Chat, Responses, and Messages streams.
- 429 with and without `Retry-After`.
- 401, 403 `MODEL_NOT_IN_PLAN`, and 5xx.
- Stalled body and cancellation.

This closes M5 and increases confidence without spending live credits.
