# OpenAI tier-switch cache experiment, 2026-10-17

## Scope

Live requests to the ChatGPT/Codex OAuth Responses backend using the configured
account and `gpt-6.1-sol`. These observations are not a guarantee for API-key
billing, other accounts, other models, or cache expiration behavior.

## Method

Two independent persistent TLS WebSocket connections, each with a fresh random
prefix followed by 500 numbered inventory records, approximately 12,000 input
tokens. Within each connection the model, instructions, user input, and reasoning
effort (`low`) were byte-identical. Only `service_tier` changed. Each request
asked for `OK`, used `store: false`, and waited for `response.completed` before
the next request, with a three-second inter-request delay.

The handshake matched Jcode's OAuth transport:

```text
originator: codex_cli_rs
OpenAI-Beta: responses_websockets=2026-02-06
```

No `previous_response_id` or `prompt_cache_key` was supplied. Each request sent
the complete identical input. This isolates upstream prompt-cache behavior from
Jcode's incremental-input and prewarm eligibility checks. Usage was taken from
`response.completed.response.usage.input_tokens_details.cached_tokens`.

## Results

### Standard first, 12,046 input tokens per request

| Request | Requested tier | Cached input tokens | Returned tier |
| --- | --- | ---: | --- |
| 1 | default | 0 | default |
| 2 | default | 11,904 | default |
| 3 | default | 11,904 | default |
| 4 | priority (Fast) | 0 | default |
| 5 | priority (Fast) | 11,904 | default |
| 6 | default | 11,904 | default |
| 7 | ultrafast | 0 | default |
| 8 | ultrafast | 11,904 | default |
| 9 | default | 11,904 | default |

A subsequent request with the literal wire value `fast` returned HTTP-style
status 400, `Unsupported service_tier: fast`, and the server closed the socket.
The nine successful measurements above were saved before that negative test.
Jcode should continue normalizing the Fast UI label to `priority` on the wire.

### Ultrafast first, fresh prefix, 12,045 input tokens per request

| Request | Requested tier | Cached input tokens | Returned tier |
| --- | --- | ---: | --- |
| 1 | ultrafast | 0 | default |
| 2 | ultrafast | 11,904 | default |
| 3 | default | 0 | default |
| 4 | default | 11,904 | default |
| 5 | priority (Fast) | 0 | default |
| 6 | priority (Fast) | 11,904 | default |
| 7 | ultrafast | 11,904 | default |
| 8 | default | 11,904 | default |
| 9 | priority (Fast) | 11,904 | default |

All nine requests in the reverse-order run completed successfully.

## Incremental continuation follow-up

A third connection used a fresh prefix, then sent each later request with
`previous_response_id` from the immediately preceding response and only a new
short user input (`Reply with only OK.`). This mirrors Jcode's incremental
WebSocket conversation shape rather than resending the full prompt.

| Request | Requested tier | Total input tokens | Cached input tokens |
| --- | --- | ---: | ---: |
| 1 | default | 12,045 | 0 |
| 2 | default | 12,061 | 11,904 |
| 3 | priority (Fast) | 12,077 | 0 |
| 4 | priority (Fast) | 12,093 | 11,904 |
| 5 | ultrafast | 12,109 | 0 |
| 6 | ultrafast | 12,125 | 11,904 |
| 7 | default | 12,141 | 11,904 |

The same first-use cold-cache pattern occurred across the incremental chain.
Every response again reported `service_tier: default`, so the same actual-tier
confirmation limitation applies.

## Application-level switching acceptance

The updated Jcode binary was run on an isolated daemon socket with real OpenAI
OAuth requests, independently of the shared daemon. A turn executed a harmless
`bash` call containing `sleep 8`. While that tool was running and the agent's
turn remained active, two consecutive protocol requests configured Fast and
Ultrafast:

| Request | Time to configuration acknowledgement | Deferred acknowledgement |
| --- | ---: | --- |
| Fast (`priority`) | 0.98 ms | No |
| Ultrafast | 1.01 ms | No |

These are single local-socket observations, not a latency distribution or a
promise of sub-millisecond performance on remote machines. The current response
retained its original requested tier, and the next provider request **within the
same turn** carried `ultrafast`. This verifies that configuration no longer waits
for the whole turn to finish, without claiming mutation of an in-flight response.

A real PTY TUI connected to that isolated daemon also passed:

- Changing to Standard and then Ultrafast produced one configuration message
  each, explicitly stating that provider acceptance was not yet confirmed.
- The persistent Runtime widget displayed the requested/unconfirmed tier.
- A short live model reply produced one warning that Ultrafast was requested
  but the provider reported `default`. There was no false Ultrafast success
  notification and no warning for the provisional `auto` metadata.
- A server-driven visual frame captured the warning and reported no anomalies.

The frozen tested binary had SHA-256
`8ba4457be7fa51f9be58b57503b2c94f09d4fdd19ca1a743c202e46eb42f9933`.
A subsequent small TUI refinement adds a transient requested/waiting status
before a server acknowledgement, without adding another transcript message.

## Interpretation and limitations

- In both runs, the first request on each previously unused requested tier was
  cold, despite a warm identical prompt on another requested tier. Repeating
  the same tier reused 11,904 tokens, approximately 98.8% of the full input.
- Returning to an already warmed requested tier retained its cache in these
  short runs. Switching did not globally destroy every tier's cached prefix.
- This is consistent with tier-separated cache routing or cache namespaces.
  It does not prove the backend's internal cache-key design. Operationally,
  budget for a cold request when first changing to an unwarmed tier.
- Crucially, every completed response reported `service_tier: default`, even
  when `priority` or `ultrafast` was requested. Thus these runs establish the
  effect of the **requested tier field** on observed cache reuse, not that the
  account actually received paid Fast or Ultrafast processing. A UI must not
  equate local configuration acknowledgement with provider-confirmed tier use.
- Earlier HTTPS probes also returned `default` for requested `priority` and
  `ultrafast`. Cache hits on that path were inconsistent even for unchanged
  tiers, so they were not used to infer cross-tier cache behavior.
- These requests were intentionally short-output diagnostics, not a latency
  benchmark. End-to-end request times do not establish relative tier speed.
- Requests were sequential. This experiment does not test changing an already
  accepted response's processing tier while it is generating.
