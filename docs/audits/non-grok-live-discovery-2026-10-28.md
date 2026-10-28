# Non-Grok runtime model discovery audit, 2026-10-28 UTC

Scope: provider runtime implementations only. No Grok runtime, shared provider/base, or app-core changes are part of this lane. Anthropic/OpenAI runtime observations were coordinated with the owner of those overlapping files. Work remained in the existing checkout on master.

## Coverage and changes

| Runtime / route | Discovery coverage | Result |
| --- | --- | --- |
| OpenRouter | Native `/models`, stale memory/disk refresh, background picker refresh, forced refresh | Already live. Fixed forced refresh bypassing `supports_model_catalog=false`. |
| OpenAI-compatible built-ins | Profile-specific authenticated `/models`, cache namespaces, built-in fallback replacement | Already live through OpenRouter runtime. No new static catalog added. |
| Custom/named compatible endpoints | User-declared static model preservation, explicit catalog opt-in/out | Explicit static-only endpoints now remain static during manual refresh without resolving auth or making an HTTP request. Existing regression tests cover live/static merge and built-in-name shadowing. |
| Gemini Developer API key | Official ListModels endpoint | Fixed first-page-only discovery, encoded opaque next-page tokens, repeated-token/page-count bounds, capability filtering, error propagation, and atomic cache update only after all pages succeed. |
| Gemini Code Assist OAuth | `loadCodeAssist` response discovery | Already live. Its metadata extraction and static fallback remain unchanged. |
| Copilot | Authenticated `/models`, picker flags and reasoning metadata | Fixed explicit model environment override disabling discovery. Pinned selection is retained while discovery still runs. Explicit refresh bypasses startup grace and propagates fetch/auth errors. Best-effort initialization wrapper still completes readiness on failure. |
| Cursor API key / OAuth | Key endpoint, AgentService OAuth catalog, bearer fallback | Already live. Failed discovery now returns an error rather than falsely reporting success. Warm catalog remains available. |
| Antigravity | `fetchAvailableModels`, project retry/fallback | Already live. Failed discovery now returns an error rather than falsely reporting success or triggering a successful-refresh notification. Warm catalog remains available. |
| Bedrock | SDK foundation-model discovery and inference profiles | Fixed single-page inference profile discovery by using the SDK paginator. A later-page failure now preserves the complete prior catalog instead of publishing a partial prefix. |
| Anthropic API key / OAuth | Authenticated live model catalog, scoped credentials, custom-gateway protection | Inspected, not modified in this lane. Sent coordinator the observation that route errors are logged and swallowed by prefetch and that explicit refresh must be checked against the scoped refresh guard. |
| OpenAI platform / ChatGPT-Codex | Credential-shaped live catalog selection and auth-refresh recovery | Inspected, not modified in this lane. Already live, with provider-owner work coordinated separately. |

Gemini protocol reference checked against the official Google AI ListModels API documentation at `https://ai.google.dev/api/models`: `nextPageToken` is sent back as `pageToken`; `supportedGenerationMethods` identifies `generateContent` support. Missing capability metadata remains accepted for compatible proxies.

## Regression evidence

All Rust checks/tests used hosted justrust. `justrust show` confirmed `where remote on hosted` for the provider regression run.

- `justrust check -p jcode-provider-gemini-runtime -p jcode-provider-openrouter-runtime`: passed.
- `justrust check -p jcode-provider-copilot-runtime -p jcode-provider-cursor-runtime -p jcode-provider-antigravity-runtime -p jcode-provider-bedrock`: passed.
- `justrust check -p jcode-provider-bedrock --features aws-sdk`: passed.
- OpenRouter catalog merge regression group: 7 passed, including disabled-catalog auth/network suppression.
- Gemini new local HTTP regression: passed. Exercises two pages, opaque token encoding, chat capability filtering, auth failure, repeated pagination token, and retained warm cache after failures.
- Gemini complete library suite: 40 passed, 1 failed. The unrelated pre-existing `build_tools_keeps_required_when_the_object_declares_no_properties` expectation fails at gemini_tests.rs:921: expected `required: ["anything"]`, actual null after schema sanitization. This lane does not modify schema normalization.
- Copilot/Cursor/Antigravity complete library suites: 120 passed. New regressions cover pinned Copilot forced discovery during startup and error propagation/cache retention for Cursor and Antigravity.
- Bedrock complete library suite with `aws-sdk`: 30 passed, 1 ignored. Uses a real SDK client pointed at a local HTTP fixture with synthetic credentials. Regression exercises foundation-model fetch, both inference-profile pages, propagation of `nextToken`, and preservation of the warm catalog after a later-page 403.

Representative hosted run ids: `20261027-231440770-3842852` (OpenRouter), `20261027-231917145-3864042` (Gemini regression), `20261027-232345812-3882563` (Bedrock full suite and SDK pagination), `20261027-232216783-3876421` (120 runtime tests).

## Limitations

- No paid inference or authenticated production catalog requests were made. Tests use loopback HTTP, deliberately closed local proxies, or isolated invalid credential fixtures. This validates runtime behavior, not the current contents or permissions of every live account.
- No shared daemon reload or end-user TUI picker workflow was performed in this lane. Shared refresh pipeline and runtime integration belong to the coordinated owner.
- Native Gemini OAuth discovery still depends on the metadata actually exposed by Code Assist. Static fallback is intentionally retained when discovery yields no usable catalog.
- Static-only custom providers remain intentionally non-discoverable until `model_catalog` is enabled. Azure deployments and arbitrary endpoints need their explicit configuration contract; a universal `/models` assumption is not imposed on them.
- Bedrock without `aws-sdk` remains cache/static-only and reports unsupported live refresh. First-page inference-profile permission failures remain best-effort so foundation-model discovery still works for accounts without ListInferenceProfiles permission.
- No claim is made that every discovered model is entitled, enabled in the current region, tool-capable, or invokable. The provider remains authoritative at request time.
- Hosted justrust reports disabled build slots after its reflink probe and downstream rebuild cost from Bedrock through jcode-base. For example run `20261027-232216783-3876421` attributed about 5.6 seconds to downstream rebuilds. No unrequested crate restructuring was performed.
