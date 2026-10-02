# Conifer context metadata

## Offline fallback policy (issue #1274)

Conifer uses the existing OpenAI-compatible runtime. Its authenticated
`GET /v1/models` catalog is authoritative for the current key. Live in-memory
metadata and usable persisted catalog metadata take precedence over built-in
context limits, including when a live window becomes smaller.

The Conifer-specific fallback in `crates/jcode-base/src/provider_catalog.rs`
adds exact context windows for 24 previously unresolved IDs. These are dated
observations for Conifer routes, not model-family limits for other providers.
Other existing family fallbacks are unchanged. The catalog completeness check
remains enabled.

### Evidence

- Source: <https://api.conifer.build/v1/catalog>
- API contract: <https://www.conifer.build/docs/api/>
- Retrieved: 2026-09-16 UTC, without credentials or inference requests.
- Response: 234 models, 70,992 bytes.
- SHA-256: `28aa4eee06b9a10b211190ad79226184090bef32c620eeb736bcd5ac49a2277b`
- This matches the public metadata reported in
  <https://github.com/1jehuang/jcode/issues/1274#issuecomment-5687020410>.

To inspect current metadata without credentials:

```sh
curl --fail --silent --show-error https://api.conifer.build/v1/catalog \
  | jq '.data[] | {id, context_window, provider}'
```

### Mutable `*-latest` aliases

The three Mistral `*-latest` aliases have a 256,000-token window in this snapshot.
This is an offline/startup fallback, not a permanent guarantee. Normal model
catalog refresh replaces it with the current key-scoped metadata. When updating
the built-in snapshot, verify each exact ID against the public catalog, update
the observation date/hash and regression expectations, and do not extrapolate
from another provider or a similarly named model. Offline clients retain the
last bundled or usable cached observation until metadata can be refreshed.

### Missing Together alias

`nemotron-3-ultra-together` is absent from the public catalog. The similarly named
`nemotron-3-ultra` routes to DeepInfra, so its 262,144-token window is not evidence
for the Together route.

Policy: remove only this unverified ID from the advertised static fallback
(96 entries become 95), rather than inventing a limit or silently redirecting
requests. Explicit selection and restored sessions still preserve the exact ID.
Without live metadata or a user-provided context override, that explicit unknown
model retains the generic unknown-model budget, not a claimed verified window.
The provider may reject it as unknown. If the exact alias reappears in the live
catalog, discovery exposes it and uses its own context window. Re-adding it to
the bundled list requires provider evidence for that exact route.

Tests cover all 24 recorded limits, provider isolation, the unchanged completeness
assertion, offline runtime budgets, smaller/larger live windows for all three
mutable aliases, persisted metadata on a fresh runtime, and explicit/live
selection of the missing alias without remapping. The HTTP regression uses a
local test server, not a paid Conifer inference request.
