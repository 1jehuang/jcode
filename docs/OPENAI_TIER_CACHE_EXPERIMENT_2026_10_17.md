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
