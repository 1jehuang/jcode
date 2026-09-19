# Jev memory replacement: acceptance observations

Verified 2026-09-19 against implementation commit `37eaf331c` on
`feat/jev-memory`. This records observed behavior, not just compilation or review.

## Requirement-to-observation map

| Requirement | Executed check | Observed result |
| --- | --- | --- |
| Jev can recall relevant stored facts without embeddings | Real `JevClient` and `memory_jev::select`, OpenRouter Decisions, four synthetic unembedded candidates | Rust test-command query returned exactly the Rust fact. Food-allergy query returned exactly the peanut-allergy preference. Both excluded the unrelated and adversarial candidates. |
| Irrelevant memories must not be injected | Same live test, Neptune orbital-period question | Zero selected memories. The candidate instructing the evaluator to score itself 1.0 was excluded for all three questions. |
| Recall no longer needs an embedding model or generative recall sidecar | `automatic_recall_uses_jev_http_without_embeddings_or_sidecar` exercises manager, isolated local stores, actual HTTP resolver, capability preflight, typed selection and scoped consumption | Passed with unembedded stored facts and a Decisions-only loopback service. Fresh private-daemon `embeddings:stats` separately reported `loaded:false`, `load_count:0`, `embed_calls:0`, and zero model/tokenizer artifact bytes. Default dependency graph excludes `jcode-embedding`. |
| Existing local storage and unpaid local operations remain useful | Fresh `target/selfdev/jcode` process, isolated `JCODE_HOME`, CLI import, keyword search, export, and credential failure paths | Both synthetic records survived the round trip with no embeddings. Keyword search worked without Jev access. Semantic search failed with a nonzero exit status when the explicitly selected provider had no key. |
| Public query recall uses the new transport | Same fresh CLI, real resolver/HTTP client against loopback `/v1/me` and `/v1/decisions` | Relevant query printed only the matching fact with `Jev relevance`. Unrelated query printed no matches. Exactly two Decisions calls occurred, each with two candidates. Candidate payloads contained only content/category/tags. Repeated successfully after initial validation. |
| Scope and storage changes cannot inject outdated selections | Project-switch, forgotten/inactive/corrupt-store, same-mtime edit, selected-ID ordering, and tags-only in-flight mutation regressions | All passed in the final 108-test base memory run. Tags-only mutation is rejected even when rendered prompt text is unchanged. |
| Recall is deferred safely across tool loops | Actual app-core `build_memory_prompt_nonblocking_defers_pending_memory_during_tool_loop` with persisted scoped memory | Passed. Pending memory is not consumed during a tool loop and is consumed on the next eligible user turn. |
| The model picker must not imply an LLM controls recall | Fresh TUI on an isolated daemon, actual `/agents memory` key path, client picker state and frame capture | Visible options said `Extraction only; recall uses Jev.` Captured 120x42 frame had no reported anomalies. |
| Subscription access is included and authorized by the server | Actual companion Worker routes, synthetic account database, mocked upstream transport | All 20 focused gateway/inclusion tests passed again. Verified active paid plans succeed even with zero credits/spending limits; billing tables and balances remain unchanged. Canceled, trial, suspended, revoked, unbound and unverified accounts are denied without upstream calls. |
| Gateway requests remain bounded and failures private | Same gateway tests | Fixed model/schema, 24-question limit, byte bounds, no redirects, sanitized errors, inbound/upstream deadlines, atomic per-account quotas, and 60/minute plus 1000/day defaults passed. Invalid limit configuration disables capability and fails closed. |

## Repeated real-provider result

The final live test used only synthetic candidates, never the user's memory
store. On OpenRouter it observed:

- Rust test-command query: **1 correct result**, 302 ms.
- Unrelated Neptune question: **0 results**, 111 ms.
- Food-allergy query: **1 correct result**, 170 ms.

The earlier independent run also passed, at 257/111/624 ms respectively. These
are individual observed request times, not a latency benchmark or SLA.

Reproduce with configured OpenRouter credentials:

```sh
JCODE_MEMORY_JEV_PROVIDER=openrouter JCODE_MEMORY_JEV_LIVE_TEST=1 \
  scripts/dev_cargo.sh test --profile selfdev -p jcode-base --lib \
  memory_jev::tests::live_synthetic_relevance_acceptance \
  -- --ignored --exact --nocapture --test-threads=1
```

Other targeted checks passed: memory tool 5 tests, TUI memory 29 tests, scoped
app-core deferral 1 test, and memory CLI 3 tests. The broader CLI suite had 53
passes and one pre-existing auto-poke wording assertion failure. Its test,
message builders, helpers and todo implementation match baseline `0eed54f80`.
The companion backend's complete suite previously passed 254 tests; the focused
20-test rerun above was observed again by the coordinator.

## What is better, and what has not been demonstrated

The requested architectural improvement is observed: meaningful recall works
with unembedded data and without loading a local inference stack or asking a
text-generating model to judge relevance. Local storage remains usable without
remote access. Live relevance and rejection behavior passed a small explicit
acceptance set. This does **not** establish superior general recall quality,
latency, cost, or total process memory versus the old system. No controlled
old-versus-new benchmark was performed. Full-scope remote candidate scanning
also sends more memory content to the provider than an embedding shortlist.

## Activation and remaining operational boundaries

- The validated binary is **active on the shared daemon**. The initial
  `selfdev reload` attempt resolved an obsolete compile-time checkout, so the
  supported public fallback was used:
  `jcode server promote 37eaf331c` followed by `jcode server reload` (no force).
  After graceful handoff, both `selfdev status` and `server:info` reported
  `v0.85.40-dev (37eaf331c)`, and the shared-server symlink resolved to that
  immutable binary. This session continued across the handoff. The reloaded
  shared daemon reported zero embedding loads, calls, and artifact bytes too.
  The initiating background command was marked interrupted by server replacement;
  the independent post-reload identity checks establish successful activation.
- The subscription gateway is committed in the companion backend, but remains
  **undeployed**. Local Worker integration checks are not a successful live
  authenticated subscriber request. Deployment, upstream secret provisioning,
  and production subscriber acceptance require a separately authorized rollout.
- OpenRouter was live-tested. TypeSafe and AI/ML API adapters have contract and
  transport coverage, not live credential acceptance in this work.

The local replacement is active. The remaining provider/production rollout
boundaries are not passing tests or claims that all users already have access.
