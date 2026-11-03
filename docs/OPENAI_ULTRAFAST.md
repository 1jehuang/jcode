# OpenAI Ultrafast

Verified against official OpenAI documentation on October 16, 2026.

## Opt in

Select an eligible OpenAI model, then run `/fast ultra` (or `/fast ultrafast`).
This changes the current session only, not the saved default.
Use `/fast` to inspect the current tier, `/fast on` for Fast, and `/fast off`
for Standard. Alt+Up and Alt+Down step through Standard, Fast, and Ultrafast
for supported models, in local and remote sessions.

To opt in persistently, set this in your Jcode config:

```toml
[provider]
openai_service_tier = "ultrafast"
```

Alternatively, launch Jcode with `JCODE_OPENAI_SERVICE_TIER=ultrafast`.
The existing default stays `priority` (Fast), not Ultrafast.
`/fast default on|off` controls the Fast default, not the Ultrafast default.
The provider accepts `ultrafast`, `ultra`, and `ultra-fast`, normalizing all
three to the exact wire value `service_tier: "ultrafast"`.

## Models, transports, and access

The current API guide lists `gpt-6-astra` and `gpt-6.1-sol`. Jcode offers
Ultrafast for these models and their snapshot/context variants. Existing
`gpt-5.6-sol` preview support is retained: the Responses reference still
lists this older model, while the current guide lists the newer models.
Model selection remains subject to your account's model catalog.

Both API-key Responses requests and ChatGPT/Codex OAuth requests carry the
tier. Ultrafast works over HTTP and WebSockets. OpenAI recommends persistent
WebSockets to reduce overhead across tool calls. Selecting Ultrafast does not
force a transport change. Explicit Ultrafast selection is rejected for
unsupported models. If a saved override or retained tier is Ultrafast after
switching to an unsupported model, the request omits that override rather
than sending an invalid tier. Returning to a supported model in the same
provider restores it. Forks copy the effective tier, so a fork made while on
an unsupported model does not inherit that dormant override.

API Ultrafast is available to all API users on the documented models, with
separate rate limits and higher per-token charges. Codex is different:
Ultrafast for Astra and Sol requires Pro $500 or eligible Enterprise/Edu
access and applicable workspace permissions. Plus, Pro $100/$200, and Business
do not gain Ultrafast just by purchasing credits. Jcode cannot infer all
server-side entitlements from a model ID. The tier UI reports model support,
not an entitlement guarantee, and server errors still apply.

## Cost estimates

Sol short-context reference prices per million tokens are:

| Tier | Input | Cached input | Output |
| --- | ---: | ---: | ---: |
| Standard | $2 | $0.10 | $10 |
| Fast | $4 | $0.20 | $20 |
| Ultrafast | $12 | $0.60 | $60 |
| Flex | $1 | $0.05 | $5 |

These are API reference estimates, not Codex subscription credit rates or a
billing guarantee. Long-context rates, cache writes, regional uplifts, and
workspace terms can increase actual charges. The route estimator stores
short-context input/output/cache-read rates, including for `[1m]` models,
and does not encode a separate cache-write price or entitlement policy.

## Official sources

- Ultrafast model availability, exact tier value, HTTP and WebSocket examples:
  <https://developers.openai.com/api/docs/guides/ultrafast-mode>
- Responses tier reference (including legacy preview availability):
  <https://developers.openai.com/api/reference/resources/responses/methods/create>
- API prices:
  <https://developers.openai.com/api/docs/pricing?latest-pricing=ultrafast>
- Codex plan and workspace restrictions:
  <https://help.openai.com/en/articles/20001275-chatgpt-work-and-codex>
