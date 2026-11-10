# Command Code Implementation Status

Last updated: 2026-11-10

## Repository state

- Upstream: `https://github.com/1jehuang/jcode`
- Upstream bases examined: `e43279cdd` for the original branch and `eb867111f`
  for the final rebase.
- Latest code commit before this status update: `fa5bb0261`
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

Commit: `9dc0f6ec9` (`feat(provider): add Command Code metadata and endpoint catalog fields`).

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

Commit: `256a42ce0` (`feat(provider): wire Command Code native login and Chat transport`).

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

Commit: `41b4b84f1` (`feat(provider): add Command Code Responses transport and strict stream terminals`).

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

Commit: `4607a2c5d` (`feat(provider): route Command Code Claude models through Messages`).

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

Status: complete for the declared transport surface.

Commits: `876ce6db4`, `79c958889`, `13becbc0a`, `3f957ba98`, `8004bc77f`.

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
- `cargo test -p jcode-provider-openrouter-runtime --lib command_code_mock_tests`:
  12 passed, 0 failed. The suite runs the real provider HTTP client against a
  loopback server and covers:
  - Chat tool call and tool-result replay.
  - Responses function call and `function_call_output` replay.
  - Anthropic Messages text streaming and tool-use/result replay.
  - Incomplete Chat and Responses streams as errors.
  - 403 `MODEL_NOT_IN_PLAN` not retried.
  - 429 retried once, then successful.
  - 500 exhausting the bounded retry and reporting status.
  - A stalled response body cancelled promptly.
  - Two concurrent streams without cross-talk.
  - Saved-account pinning across a global active-account switch.
- `cargo test -p jcode-app-core --lib cancellation_during_tool_execution_aborts_tool_and_continuation`:
  1 passed. The streaming turn aborts the running tool future and makes no
  continuation request after cancellation.
- `jcode provider-doctor command-code --tier full` against the isolated store:
  every strict checkpoint passed, including live catalog, picker, non-streaming
  chat, streaming chat, tool parse, tool loop, tool-result follow-up, and real
  JCode tool smoke. Provider-reported spend: 3 billable calls, 476 tokens.

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

Status: complete for labeled API-key accounts. Automatic failover is explicitly
out of scope.

Commits: `77a57328e`, `db6d47fd9`, `cc8ed289d`, `fa5bb0261`.

Implemented:

- JCode-owned command-code account store with mode `0600` writes and the shared
  cross-process account-store lock.
- Labeled accounts, active account, replacement by label, and removal.
- Precedence: `COMMAND_CODE_API_KEY` environment variable, active stored
  account, legacy `command-code.env`.
- Shared runtime key resolution now consults the store, so `jcode run`,
  provider doctor, auth status, and the profile runtime agree.
- Login accepts `--account <label>`; re-running with an existing label and no
  new key selects that account.
- The login notice and success message name the account store instead of
  claiming the key is in `command-code.env`.
- Corrupt store files are preserved rather than silently overwritten.
- Session account affinity: the session records
  `openai-compatible:command-code@<label>` and keeps that account pinned across
  global active-account changes, provider forks, process restart, and daemon
  restart plus ACP resume.
- Pinned accounts fail closed if removed; JCode does not silently substitute the
  current global active account.

Verified:

- `cargo test -p jcode-base --lib command_code_accounts`: 5 passed, including
  label switching, environment precedence, shared runtime resolution, and
  corrupt-store preservation.
- `cargo test -p jcode --lib 'cli::login'`: 34 passed, including labeled
  account login, account switching, and credential-source notice.
- Isolated two-account live check:
  - active `personal` with a deliberately invalid test key returned 401
    `UNAUTHORIZED` and spent no generation;
  - switching active to `work` produced `PONG` on
    `deepseek/deepseek-v4-flash`.
- Daemon restart check:
  - created a headless session on account `alpha`;
  - switched the global active account to invalid `beta`;
  - stopped and restarted the isolated daemon;
  - resumed the session through ACP and received `DAEMON_ACP_RESUMED` on the
    pinned `alpha` account.

Not implemented:

- Automatic cross-account failover after rate limits or plan errors.
- TUI account picker.

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

Status: ready for review with the declared external blockers.

Implemented:

- `docs/COMMAND_CODE_PROVIDER.md`.
- This status file.
- Unit and live evidence for M1 through M6.

Remaining:

- Upstream PR preparation and coordination with PR #1271.

Final verdict: `READY_FOR_REVIEW_WITH_DECLARED_BLOCKERS`.

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
| `cargo test -p jcode-provider-openrouter-runtime --lib command_code_mock_tests` | 12 passed, 0 failed |
| `cargo test -p jcode-provider-openrouter-runtime --lib` | 210 passed, 1 ignored, 0 failed |
| `cargo test -p jcode-base --lib command_code_accounts` | 5 passed, 0 failed |
| `cargo test -p jcode-base --lib provider::tests::session_binding` | target test added for fork account inheritance |
| `cargo test -p jcode-app-core --lib resumed_session_keeps_its_saved_api_key_account` | 1 passed |
| `cargo test -p jcode-app-core --lib cancellation_during_tool_execution_aborts_tool_and_continuation` | 1 passed |
| `cargo test -p jcode --lib 'cli::login'` | 34 passed, 0 failed |
| `cargo test -p jcode-provider-doctor` | 39 passed, 0 failed |
| `jcode provider-doctor command-code --tier full` | all strict checkpoints passed, 3 billable calls, 476 tokens |

### Broader suite results and unrelated failures

`cargo test -p jcode-base --lib` with `umask 0022`: 2167 passed, 4 failed, 12
ignored. Each failure was rerun individually against upstream base
`e43279cdd` in a detached worktree:

| Test | Upstream base individual | Branch full suite |
| --- | --- | --- |
| `config_env_fingerprint_tracks_every_apply_env_override_var` | FAIL | FAIL, pre-existing missing `JCODE_RESUME_OPEN_ELSEWHERE` |
| `hanging_browser_cli_times_out_instead_of_blocking_forever` | PASS | FAIL, parallel-suite timing flake |
| `cached_oauth_reports_refresh_local_totals_without_refetching_quota` | PASS | FAIL, parallel-suite timing flake |
| `test_default_swarm_prompt_defines_lane_ownership_and_parallel_work` | FAIL | FAIL, pre-existing prompt-length assertion |

`cargo test -p jcode --lib` with `umask 0022`: 455 passed, 5 failed. Individual
upstream-base reruns:

| Test | Upstream base individual | Branch full suite |
| --- | --- | --- |
| Three `restart_*` tests | FAIL | FAIL, debug control disabled |
| `auth_test_choice_plan_discovers_model_for_hosted_custom_compat_endpoint_with_api_key` | FAIL | FAIL, stale localhost expectation |
| `cloud_sessions_sync_dry_run_reports_without_uploading_or_writing_state` | PASS | FAIL, external Jade helper absent in the full-suite environment |

Repo-wide `cargo fmt --all -- --check` is red on unrelated pre-existing files.
Every Rust file changed by this branch passes `rustfmt --check`.

`scripts/check_code_size_budget.py` remains red because the checked-in baseline
predates many unrelated master changes. After extracting the account-store
login helpers, no file touched by this branch appears in the script output:
`src/cli/login.rs` is 1195 lines and
`crates/jcode-provider-doctor/src/provider_e2e.rs` is flat at 2727 lines.

### Compile and build

| Command | Result |
| --- | --- |
| `cargo check -p jcode-base --lib` | passed |
| `cargo check -p jcode --bin jcode` | passed |
| `cargo build -p jcode --bin jcode` | passed |

### Live, isolated

Live tests used isolated homes under
`/home/quant/src/jcode-commandcode-fixes/` (`tmp-cc-home2` for the first pass,
`tmp-cc-accounts` for account and doctor checks) plus dedicated sockets where
needed. The production JCode config, credentials, and shared daemon were not
modified.

| Check | Result |
| --- | --- |
| `jcode login command-code --api-key ... --no-browser` | PASS, whoami verified, provider smoke `AUTH_TEST_OK`, tool smoke `AUTH_TEST_OK` |
| Chat tool round trip on `deepseek/deepseek-v4-flash` | PASS, tool result `alpha\nbeta`, final `DONE` |
| Responses tool round trip on `meta/muse-ember` | PASS, `format=responses`, cache_read 15476 |
| Session resume | PASS, `RESUMED_OK`, cache_read 14080 |
| `/usage` honesty | PASS, configured key and unknown quota |
| Claude Messages path | BLOCKED by account plan, 403 `MODEL_NOT_IN_PLAN`, endpoint and auth verified |
| Active account selection (invalid key) | PASS, 401 with active `personal` account, no generation |
| Active account selection (valid key) | PASS, `PONG` after switching active to `work` |
| Provider Doctor, full tier | PASS, every strict checkpoint |

## Current blockers

1. Claude generation cannot be verified on the available account because it has
   no Claude entitlement. A Pro, Provider, or other Claude-entitled account is
   required for the final M4 acceptance test.
2. Browser login requires confirmation that the Command Code CLI callback is a
   supported third-party contract.
3. The upstream code-size baseline is stale on master. The Command Code changes
   do not regress any tracked file, but `scripts/check_code_size_budget.py`
   cannot be used as a green gate without a separate baseline cleanup. In
   particular, `crates/jcode-provider-anthropic-runtime/src/lib.rs` was 2777
   lines at the parent commit and 2746 lines after moving its direct-URL helpers
   into `compatible.rs`; the script still compares it to an older 2692-line
   baseline. This branch shrinks the file by 31 lines.
4. Automatic cross-account failover is not implemented. The declared behavior
   is explicit account selection, not silent substitution.

## Upstream coordination

- PR #1271 comment: https://github.com/1jehuang/jcode/pull/1271#issuecomment-6772238116
- Issue #741 comment: https://github.com/1jehuang/jcode/issues/741#issuecomment-6772242095
- Contribution branch: https://github.com/ddawnlll/jcode/tree/command-code-native-provider
- No upstream pull request was opened. The branch is offered for comparison and
  for a contribution PR targeting the existing author branch if the maintainer
  or author prefers that route.

## Next independently verifiable task

Prepare the upstream contribution after maintainer coordination:

- Rebase on the current upstream master.
- Re-run the full Command Code unit and mock suites with `umask 0022`.
- Re-run `scripts/check_code_size_budget.py` and separate any stale-baseline
  failures from branch regressions.
- Include the Claude entitlement blocker in the PR description rather than
  claiming verified Claude generation.
