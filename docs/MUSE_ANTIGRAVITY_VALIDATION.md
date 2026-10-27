# Muse subscription and Antigravity repair evidence

Validation date: October 27, 2026. Base: upstream `master` at `fea0814da`.
Related issues: #1991 (Muse availability), #1992 (Antigravity).

## Implementation rationale

| Change | Why it is needed and evidence | Checks |
| --- | --- | --- |
| Make Muse subscription available without an environment opt-in | The native device-login, credentials, subscription checks and inference already work. A separate flag prevented normal users from selecting their working subscription. A real native inference request succeeds with the variable absent. | Muse protocol/catalog tests run with the opt-in removed. Compatibility tests cover absent and legacy flag values. An ignored live test requests native Muse inference without the variable. |
| Remove experimental wording from Muse menus, login, status, CLI and documentation | Removing the gate alone leaves contradictory instructions and misleading availability status. | Metadata and TUI authentication tests, provider matrix, native CLI acceptance. |
| Retain Muse security and third-party/billing warnings | Removing a Jcode rollout flag does not establish vendor endorsement or guarantee subscription billing terms. Host binding, official-profile credential isolation, active-subscription checks and payment refusal must remain intact. | Existing sandbox protocol and catalog safety tests retained and run without the flag. No payment is attempted. |
| Refresh Antigravity client version to the locally verified `2.5.5` baseline and share it with authentication | Controlled requests with the same account, endpoint, model and prompt returned HTTP 404 with `1.18.3` and HTTP 200 with `2.5.5`, both without thinking configuration. Independent version constants could diverge again. The existing override remains available. | Version/default tests, controlled live A/B, direct native inference. This is a known-good baseline, not a claim that 2.5.5 is the latest distributed IDE release. |
| Expose Gemini 3.8 Flash low, medium, high and tiered routes | Live discovery can return only `gemini-3.8-flash-tiered`, hiding usable effort variants from selection. Direct calls confirm the variants work on this account. | Discovery and cached-catalog expansion tests, no-duplicate behavior, quota/context/availability preservation and live three-tier inference. |
| Send native `thinkingLevel` and `includeThoughts` | A model name alone does not request visible reasoning summaries. The same accepted high-tier request without configuration returned only answer text, while explicit configuration returned `thought: true` parts. | Request serialization and live high-effort thought-event assertions. Optional shared request fields remain absent for unrelated direct Gemini requests. |
| Route thought parts to `ThinkingDelta`, not answer text | The previous parser treated all text as answer output. Providers explicitly mark thought summaries. | Shared parser tests for thought versus answer events, preserving existing thought-signature behavior. Reasoning-only output cannot count as a usable answer or suppress malformed-response handling. |
| Preserve reasoning in CLI NDJSON output | End-to-end inspection found that the CLI serializer silently discarded `ReasoningDelta`, even after the provider emitted correct thought events. This prevented scriptable clients from showing thinking. | Serializer regression tests check event order, newline escaping and separation from final answer text. Rebuilt isolated CLI high-effort acceptance checks an actual `reasoning_delta`. |
| Do not silently substitute Flash for unavailable Pro routes | Retired or unauthorized Pro routes return 404 even with the newer client fingerprint. Replacing a user's selected family with Flash would misrepresent the selected model. | Model-remapping tests retain explicit model IDs. Bare Gemini 3.8 Flash resolves to its medium preset. |
| Lower nullable JSON Schema type arrays for Gemini's scalar protobuf schema type | The first full CLI test, with the real default tool registry, failed HTTP 400 at nested todo fields such as `type: ["string", "null"]`. Successful no-tool probes were insufficient. | Shared dialect tests, conformance rejection of surviving arrays, direct Gemini and Antigravity builder regression tests, real application tool registry sweep and rebuilt CLI tool round trip. |
| Preserve schema meaning and other providers | Scalar `T` plus `nullable: true` represents `T` or null. Multiple non-null alternatives retain their constraints in typed `anyOf` branches. Schema-valued fields are traversed, literal defaults are not rewritten. | Deep nesting, constraints, literal preservation, duplicate alternatives, idempotence and unchanged Anthropic type unions. The transformation is enabled only for the Gemini dialect. |
| Keep live network tests ignored by default | Normal CI must not depend on personal OAuth credentials or consume paid quota. | Explicit native live tests are opt-in. Offline coverage remains independent of credentials. |

## Primary-source comparison

The actual 9router Antigravity implementation was inspected, not its Vertex adapter:
`decolua/9router` at `3a978a631ad65529c06e48ce495092d5bba29f46`, specifically
`open-sse/providers/registry/antigravity.js`,
`open-sse/translator/concerns/thinkingUnified.js`, and
`tests/unit/gemini-38-integration.test.js`.
It uses model effort aliases and native thinking configuration. Jcode's implementation
uses its existing native OAuth provider, not a required external gateway.

## Scope and limitations

- This removes Jcode's Muse experimental rollout restriction, not third-party service or billing caveats.
- No shared user daemon, active `jcodemuse` process or installed binary is replaced during validation. CLI checks use an isolated socket and a freshly built binary.
- Gemini low effort may legitimately emit no thought summary. High effort is checked for actual thought events.
- Antigravity still uses its existing unary HTTP response transport. Thought parts become distinct UI events after the response arrives, not true first-token network streaming.
- Newer client fingerprints did not make retired Pro or unavailable Argon routes work on this account. Model visibility and authorization depend on the service and account.
- PR #1892 is deliberately out of scope.

## Validation record

Observed final results on Linux, October 27, 2026:

| Validation | Result |
| --- | --- |
| Six affected provider/schema library suites | 307 passed: Antigravity 11, Antigravity runtime 35, Gemini types 9, metadata 19, compatible runtime 191, schema dialect 42. Three opt-in/retry tests ignored in this offline run. |
| Muse auth and catalog (`cargo test -p jcode-base --lib muse`) | 30 passed |
| TUI authentication (`cargo test -p jcode-tui --lib auth::`) | 31 passed |
| Root CLI login / provider initialization | 31 / 24 passed |
| CLI NDJSON tests | 3 passed, including reasoning versus answer separation |
| `every_provider_sends_clean_schemas` wire integration | 9 passed |
| Real app tool registry dialect sweep | 1 passed |
| Root `provider_matrix` integration | 9 passed |
| Explicit ignored native live tests | Muse without opt-in passed. Gemini 3.8 low, medium, high all passed, with high-effort thinking. |
| Fresh full CLI with default tools | Seven cases passed with correct provider/model identity and final answers. Low, medium and high returned 1813. High and medium emitted `reasoning_delta`. High completed a real read-tool round trip and answered `jcode`. Antigravity Claude and GPT OSS returned 391. Muse returned 391 with the opt-in absent. Low emitted no thought summary in this run, which is valid. |
| `cargo build --bin jcode` | Passed |
| Formatting / patch whitespace | `cargo fmt --all -- --check` and `git diff --check` passed |
| Full Gemini runtime library suite | **39 passed, 1 failed**, the existing `build_tools_keeps_required_when_the_object_declares_no_properties` contradiction tracked in #1951. Not reported as green and not hidden by deleting or weakening the test. |

The full affected offline library command was:

```sh
cargo test -p jcode-schema-dialect -p jcode-provider-antigravity \
  -p jcode-provider-antigravity-runtime -p jcode-provider-gemini \
  -p jcode-provider-metadata -p jcode-provider-openrouter-runtime --lib
```

The live provider checks can be rerun with a configured account:

```sh
env -u JCODE_EXPERIMENTAL_MUSE_SUBSCRIPTION cargo test \
  -p jcode-provider-openrouter-runtime live_native_muse -- --ignored --nocapture
cargo test -p jcode-provider-antigravity-runtime live_gemini_38 \
  -- --ignored --nocapture
```

CLI checks use `target/debug/jcode --no-update --no-selfdev --socket <fresh-isolated-socket>`
with explicit provider/model selection and `run --ndjson`. The seven cases exercise all three
Flash presets, a read-only tool call with follow-up, Claude, GPT OSS and native Muse.
Assertions inspect the actual `done` route and answer, high-effort `reasoning_delta`, and
successful `tool_done`, not just exit codes. Failed intermediate builds and the original
full-tool HTTP 400 were not treated as successful validation.
