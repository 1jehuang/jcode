# Native Muse subscription login investigation and testing

Date: October 18, 2026

## What the user asked for

Use a Muse Code subscription through Jcode's `/login` without installing or running the Muse CLI. Compare the existing Claude, OpenAI and Antigravity implementations, keep subscription and API billing distinct, and show usage. Coding workers use Muse Spark through OpenCode Go, not the Meta provider.

## Why the process-backed adapter was rejected

An earlier proposal drove `muse serve` over the Muse Session Protocol (MSP): a process adapter that lets the `muse` binary own credentials, tools and its agent loop, and displays that process's events. Its binary dependency follows from that architecture, not from OAuth itself. It is not native account authentication, and it does not satisfy a no-Muse-install requirement, so it is not part of this change.

The `muse-subscription` route added here is a separate, experimental native device-login route. It uses Jcode's ordinary compatible-provider runtime with the Responses wire. Jcode retains its own system prompt, complete supplied conversation history, tools, workspace and stream lifetime. It does not read Muse's credential files or spawn a Muse executable.

`meta-muse` remains pay-as-you-go API-key authentication, unchanged by this change. Its keys are never a fallback for `muse-subscription`. Existing aliases such as `muse` continue to mean what they meant before. Select the new route explicitly as `muse-subscription`.

## How the existing OAuth providers work in this checkout

These are descriptions of Jcode's implementation, not a statement that every vendor authorizes every third-party client.

| Provider | Native login | Renewal and credential ownership | Inference |
| --- | --- | --- | --- |
| Claude | Authorization-code browser flow with PKCE, authorization-code exchange and a public client identifier | Jcode-managed `auth.json`, account selection, refresh-token handling | Bearer OAuth on Anthropic Messages with OAuth-specific beta headers. Direct Anthropic API-key auth is a separate route. |
| OpenAI | Browser authorization-code flow with PKCE and state validation | Jcode-managed `openai-auth.json`, account selection and coordinated refresh-token handling | The existing subscription implementation uses the ChatGPT/Codex backend rather than treating a ChatGPT login as an ordinary API key. Direct OpenAI API-key auth is separate. |
| Antigravity | Google authorization-code browser flow with PKCE, state and a loopback callback | Jcode-managed `antigravity_oauth.json`, Google access-token refresh and project discovery | Cloud Code Assist transport with project/model discovery, not the AI Studio API-key endpoint. |
| Muse subscription, experimental | RFC 8628 device flow followed by a separate API-key mint | Jcode-owned native credential file, conditional key re-minting from its saved device grant. No claim that the device grant has a supported refresh-token mechanism. | Direct Meta Responses using the minted credential, behind explicit experimental opt-in. Subscription billing for a third-party client is not verified. |

Relevant source files:

- `crates/jcode-base/src/auth/oauth.rs`
- `crates/jcode-base/src/auth/claude.rs`
- `crates/jcode-base/src/auth/codex.rs`
- `crates/jcode-base/src/auth/antigravity.rs`
- `crates/jcode-provider-anthropic-runtime/src/lib.rs`
- `crates/jcode-provider-openai-runtime/src/lib.rs`
- `crates/jcode-provider-antigravity-runtime/src/lib.rs`
- `crates/jcode-base/src/auth/muse.rs`

The reusable architecture is login, private credential storage, renewal, explicit routing, catalog discovery and status display. Endpoint URLs, grant types, scopes, request headers and account entitlements are provider-specific. Copying another provider's OAuth code does not transfer its access rights or billing semantics.

## Important support and billing limitation

Meta's subscription documentation says the subscription credential is for Muse Code only and that subscriptions work through the Muse Code CLI. No published third-party OAuth registration or native subscription billing contract was identified in this investigation.

This implementation is therefore explicitly experimental and disabled unless the operator sets `JCODE_EXPERIMENTAL_MUSE_SUBSCRIPTION=1`. That is acknowledgement of the disclosed limitation, not proof of vendor authorization or subscription settlement. Review Meta's current terms and obtain vendor confirmation before relying on this route.

The implementation refuses a mint response without an explicitly active subscription, refuses payment-required responses, and does not substitute a pay-as-you-go key. These checks do **not** prove that a subsequent third-party inference request is covered by the subscription. Model-list success, token acquisition and even inference success are not billing receipts.

Do not interpret `available` in auth status as a live model or billing test. Native auth readiness is based on stored credential presence and local validation until a real request is tested.

## Evidence for the native protocol

1. The official installed Muse launcher contains the public device client identifier and `https://auth.meta.com/oidc/device/authorization/` and `/oidc/device/token/` endpoints. It sends `client_id`, then polls using the device-code grant. This is source evidence, not a completed login.
2. The primary source implementation of the independent `danzilberdan/opencode-muse` client uses `POST https://api.meta.ai/muse-code/key` with a device access token and JSON `{}`. Its model discovery uses `GET https://api.meta.ai/muse-code/models`. This is an observed client contract, not official third-party subscription documentation.
3. On October 18, the live mint and model endpoints both returned HTTP 401 to credential-free probes. This establishes reachability and authentication gating only.
4. Official Muse SDK documentation publishes the MSP integration and explicitly pairs its clients with a Muse binary. It is evidence for the process-backed adapter, not for binary-free subscription access.

Sources reviewed:

- https://dev.meta.ai/docs/muse-code/subscriptions
- https://dev.meta.ai/docs/authentication
- https://dev.meta.ai/docs/overview
- https://meta-models.github.io/muse-code-sdk/next/
- https://github.com/danzilberdan/opencode-muse/blob/main/src/oauth.ts
- https://github.com/danzilberdan/opencode-muse/blob/main/src/constants.ts

## Testing the native route

Use the built branch binary, not an older globally installed Jcode. The examples assume the repository root.

```sh
export JCODE_EXPERIMENTAL_MUSE_SUBSCRIPTION=1
./target/debug/jcode login --provider muse-subscription
```

For remote/manual login, use the existing scriptable begin and completion interface:

```sh
./target/debug/jcode login --provider muse-subscription --print-auth-url --json
# Open the returned verification URL and confirm the displayed user code.
# Use the resume command returned by this invocation to complete the same flow.
```

The returned prompt is not login success. Credentials are saved only after approval, key minting, subscription checks and secure persistence succeed. Do not paste access tokens or credential files into chat or issue comments. Cancelling a pending flow removes local pending state, it does not revoke an already approved server-side grant.

After native login, select the explicitly named provider in Jcode's model picker or use the native profile on the CLI. Check the binary's `--help` for the exact run and model-list command syntax. Do not test an inference turn until you accept the disclosed third-party support and billing limitations.

The default is standard `muse-spark-1.3`, not a Contributor model. Contributor model selection has content-use implications and must be deliberate. The account's live catalog supplies model identifiers and context limits. Wonder Full is not fabricated from a guessed ID or a subscription price. An advertised model still needs an actual inference turn to establish access for that request.

## Usage display

- Login/auth status shows subscription usage only when returned in an account mint snapshot. It is labelled as a last account snapshot, not a continuously refreshed quota meter.
- Native Responses subscription-usage frames are mapped to status details only when numeric percentage fields are valid. Missing usage is unknown, never zero.
- Responses token usage continues through Jcode's normal token-usage events.
- Quota percentages are not token counts or dollar-cost estimates. Neither quota nor token display proves subscription billing.

## Acceptance ledger

### Verified in this change (synthetic, loopback-only; no Meta traffic)

| Check | Command | Observed |
| --- | --- | --- |
| Native auth module unit tests | `cargo test -p jcode-base --lib muse` | 30 passed, 0 failed |
| Catalog gate tests | `cargo test -p jcode-base --lib provider_catalog_muse` | 4 passed, 0 failed |
| Provider-metadata suite | `cargo test -p jcode-provider-metadata` | 19 passed, 0 failed |
| TUI auth module | `cargo test -p jcode-tui --lib auth::` | 27 passed, 0 failed |
| Compatible-runtime suite (parallel, repeated) | `cargo test -p jcode-provider-openrouter-runtime --lib` | 187 passed, 6 consecutive runs |
| Initial provider and login suites | `cargo test -p jcode --lib provider_init` / `--lib login` | 24 and 46 passed |
| Formatting | `cargo fmt --all --check` | clean for every file this change touches |

The parallel repeat matters: this suite is flaky at the parent commit **without any of these changes** (2 of 6 pristine runs failed; 4 of 6 with the new native tests added). Every failure was a global env race, not native-auth logic: `OPENROUTER_API_KEY not found`, autodetected base mismatches, and a lost per-profile cache namespace. The trigger is a pre-existing test in `catalog_fetch.rs` that mutated the process-global `JCODE_HOME` without the shared test env lock, so it raced the autodetect and provider tests. It now takes that lock, and the suite has passed every repeated parallel run since.

One test is worth calling out because it is the only one that actually pins the credential concurrency guarantee: `refresh_holds_lock_so_logout_cannot_interleave_between_compare_and_write` runs a logout from inside `refresh_credentials`' locked critical section and asserts it cannot complete there. It was verified to **fail** when the lock acquisition is removed, which the earlier interleaving tests did not do (they inject the delete before the comparison, where the old code also refused).

### Pre-existing failures observed here, unrelated to this change

These fail in this environment on the parent commit as well, and are not caused by this change:

- `jcode-base`: six `auth::cursor::tests::vscdb_*` tests require a `sqlite3` binary that is absent here.
- `jcode-base`: `browser::browser_tests::update_stops_only_hosts_running_the_replaced_binary` depends on host process/executable state.
- `jcode`: three `cli::commands::restart::restart_tests::*` tests require `JCODE_DEBUG_CONTROL=1` or a self-dev server.
- `scripts/check_code_size_budget.py` is red on the parent commit for 52 unrelated files with a stale baseline. This change introduces **no new** size violations and clears none; it shrinks four oversized files it touches and adds 1 line to `crates/jcode-tui/src/tui/app/remote.rs`, which was already a violating file.
- `cargo fmt --all --check` reports five pre-existing offenders on the parent commit that this change does not touch (`jcode-app-core/src/tool/tests/mcp_collision.rs`, `jcode-base/src/provider/tests.rs`, `jcode-provider-openai-runtime/src/openai_tests.rs`, `jcode-provider-openrouter/src/stream_tests.rs`, `jcode-tui/src/tui/info_widget.rs`). This change adds none of its own.
- The wider `jcode-tui` lib test binary is broadly flaky in this environment. The failing set changes between runs (14, 16 and 18 failures on three consecutive runs, different test names each time) and covers areas this change does not touch: clipboard helpers, git line cache, swarm buffer rendering, model picker and session picker. `tui::app::remote::tests::history_speed_capabilities_are_authoritative_and_identity_scoped` reproduces on pristine upstream master, so it is not caused by this change. The suite is also affected by the same class of unlocked global env mutation as the runtime crate: its failures concentrate in tests that read process env while other tests mutate it.

### Not verified (requires the account holder)

A completed browser approval, a successful account key mint, a live account catalog, model inference, and billing reconciliation have **not** been exercised. They require account-holder participation and, for inference, can consume allowance or incur charges. Synthetic protocol tests cannot replace those steps, and none of the green rows above is evidence that a subscription turn is billed as subscription usage rather than pay-as-you-go.

Do not read `available` in auth status as a live model or billing test.
