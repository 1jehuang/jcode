# Prompt cache affinity investigation, 2026-10-24

Scope: `session_wolf_1792874556347_4c514d33f29679db`, local UTC-07,
`~/.jcode/logs/jcode-2026-10-24.log`, checkout `ab899bd86`.

## Observed misses

| Completion time | Input tokens | Cache read | Prior input | Next cache read |
| --- | ---: | ---: | ---: | ---: |
| 13:43:42.945 | 20372 | 4352 | 19970 | 20352 |
| 13:44:20.155 | 30743 | 20096 | 30379 | 30336 |

The corresponding requests started at 13:43:39.202 and 13:44:14.771.
Both sent one function-call output using `previous_response_id` on the
existing persistent WebSocket. There was no reconnect or full replay at either
miss. The original connection was established at 13:42:45.506.

Full-request canonical logs at 13:43:39.201 and 13:44:14.769 show an
unchanged existing input prefix, instructions hash `11718209896475448257`,
tools hash `10910653871577628278`, and prompt cache key hash
`15925431234955181976`. The actual delta logs at 13:43:39.206 and
13:44:14.776 retain those same instructions/tools/key hashes. A delta's
`prefix_matches=false` compares successive *deltas*, not reconstructed full
conversation prefixes, so it is not evidence of a full-prefix mutation.

## Code findings

This checkout has no `effective_prompt_cache_key` symbol. Its
`cache_routing_key` uses the persisted session ID unless an explicit
`JCODE_OPENAI_PROMPT_CACHE_KEY` override is configured. The same key is sent
in the body and OAuth session-id header. Persistent continuation rejects a
socket whose handshake cache key differs and copies the request key into the
serialized continuation. Session rebinds clear prewarm/socket state.

Same-session forks intentionally inherit affinity (commit `6528d19ff`), and
rebinding a child uses its independent lock. Observed sibling sessions have
different key hashes. No cross-session routing collision was established for
these misses. A shared explicit override can intentionally collapse routing
keys, but there is no evidence that it explains this session.

## Disposition and limits

No cache-routing behavior change is justified by this evidence. In particular, do not rotate
keys, reconnect after low cache reads, disable continuation, or attribute the
misses to confirmed provider eviction. Stable client routing metadata does not
prove stable provider-side execution placement or cache residency. The current
logs cannot distinguish provider scheduling/cache behavior from unobserved
server-side context reconstruction. Neither is established as the root cause.

The two-turn wire regression now verifies that real serialized continuations
preserve the cache key, instructions, and tools across tool callbacks, including
when a completion has an empty output array. This is a local synthetic server
test, not a reproduction of upstream cache allocation. Resolving the observed
misses still requires upstream routing/cache evidence or a controlled live
reproduction with provider response correlation.

## Diagnostic instrumentation

Native terminal `prompt_cache_diagnostics` logging is provided by the central
Responses parser (separate coordinator change `ddd0906a3`), shared by SSE and
fresh/reused WebSockets. This runtime change does not duplicate its parser,
framing, or logging. Cancellation drains and prewarm-only terminal events do not
pass through that central parser and are not instrumented by this change.

For API-key WebSocket continuations on GPT-5.6 and later GPT model IDs, the prior
completed response ID is passed as `prompt_cache_options.comparison_response_id`.
Existing options and an explicitly supplied comparison ID are preserved. Warmup
IDs and cancellation full-replay paths do not acquire automatic comparisons.
This is diagnostic only, not a change to the cache key or conversation chain.
No comparison option is automatically sent to OAuth. API documentation is not
proof that the ChatGPT/Codex endpoint accepts the same option, so the screenshot's
OAuth path still depends on unsolicited diagnostics or provider-side evidence.
Automatic comparison does not yet cover HTTPS or fresh-socket recovery.

Official contract consulted: OpenAI API guide, “Prompt cache diagnostics”,
sections “How it works”, “Streaming”, and “Limitations”, read 2026-10-24.

The runtime also records only the presence of `x-codex-turn-state` on transport
response headers. It does not yet replay that token: correct replay requires
application user-turn boundaries and authentication-owner resets, not simply
response boundaries. No such boundary contract was added in this provider-only
change. Missing replay may matter on reconnect/HTTPS, but the reported misses
were on the same live socket, so it is not an established explanation for them.
The replay contract is documented in official `openai/codex` commit
`da20788df913189878ebca7f4963d8a363ee6bf2`, `codex-rs/core/src/client.rs`,
`ModelClientSession::turn_state`.

## Validation and remaining outcome

Hosted validation on 2026-10-24 passed the complete application check
(`20261024-135701967-2378374`), 24 cache-monitor tests, 54 Responses-parser
tests, 13 runtime cache tests (`20261024-135854871-2385664`), and three
two-turn serialized WebSocket regressions (`20261024-135858593-2385506`).
The last group covers normal API completion, empty output completion, and
OAuth omission of unsupported diagnostic options. These are synthetic socket
fixtures, not live upstream cache-allocation tests.

This work corrects causal claims and adds evidence collection. It does not
establish or repair the upstream cause of the two observed OAuth shortfalls.
No shared daemon was restarted to deploy these changes by this investigation.
