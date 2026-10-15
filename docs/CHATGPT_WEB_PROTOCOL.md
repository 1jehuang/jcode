# ChatGPT web network protocol (observed 2026-10-15)

Findings from capturing chatgpt.com traffic for one Temporary Chat turn in
jcode's private browser profile (Chrome 145, paid workspace account). This is
an undocumented, changing interface. Treat every field name as observed, not
guaranteed.

## Summary

- **Chat does not run over websockets.** A turn is one HTTP POST to
  `/backend-api/f/conversation` that answers with a server-sent event stream.
- The websockets at `wss://ws.chatgpt.com/.../ws/user/...` are a pub/sub
  notification channel (topics such as `conversations`, `pages`,
  `resource-updates`). For a turn they only carry `conversation-created` and
  `conversation-turn-complete` notices, never answer text.
- The conversation POST needs anti-abuse headers that only the page's own
  JavaScript can produce (proof of work plus a Cloudflare Turnstile token).
  Sending without them returns `403 Unusual activity has been detected`.
- jcode therefore lets the page send the request and only rewrites its body:
  the composer gets a short placeholder, an in-page `fetch` hook swaps in the
  exact jcode prompt, and the same hook tees the response stream so jcode
  reads the raw answer as it arrives.

## Request sequence for one turn

| Step | Request | Notes |
| --- | --- | --- |
| 1 | `POST /backend-api/sentinel/chat-requirements/prepare` | Body `{"p": <client fingerprint>}`. Returns a `prepare_token`, a Turnstile challenge (`dx`), a proof-of-work `seed` and `difficulty`. |
| 2 | `POST /backend-api/sentinel/chat-requirements/finalize` | Body: `prepare_token`, solved `proofofwork`, `turnstile` answer. Returns the chat-requirements `token`, valid for 540 s. |
| 3 | `POST /backend-api/f/conversation/prepare` | Debounced while typing. Returns a `conduit_token`. |
| 4 | `POST /backend-api/f/conversation` | The turn. `text/event-stream`, gzip. |
| 5 | `GET /backend-api/conversation/<id>` | The page reloads the finished conversation. jcode's transcript read uses the same endpoint. |

Headers on step 4 (values omitted): `Authorization: Bearer <session access
token>`, `ChatGPT-Account-Id`, `oai-did`, `OpenAI-Sentinel-Chat-Requirements-Token`,
`OpenAI-Sentinel-Proof-Token`, `OpenAI-Sentinel-Turnstile-Token`,
`x-conduit-token`, `x-oai-turn-trace-id`, `OAI-Echo-Logs`,
`x-openai-web-sse-compression: gzip`, plus client identification headers.

Body of step 4 (Temporary Chat, first turn):

```json
{
  "action": "next",
  "history_and_training_disabled": true,
  "model": "gpt-6-thinking",
  "thinking_effort": "extended",
  "parent_message_id": "client-created-root",
  "messages": [{
    "id": "<uuid>",
    "author": {"role": "user"},
    "content": {"content_type": "text", "parts": ["<prompt>"]},
    "recipient": "all"
  }],
  "supported_encodings": ["v1"],
  "timezone": "Europe/Helsinki",
  "client_prepare_state": "sent"
}
```

The prompt goes in verbatim as a JSON string, so the network path has none of
the composer's Markdown re-escaping or paste-to-file limits.

## Stream format (`supported_encodings: ["v1"]`)

```
event: delta_encoding
data: "v1"

data: {"type":"resume_conversation_token", ...}

event: delta
data: {"p":"","o":"add","v":{"message":{...},"conversation_id":"..."},"c":0}

event: delta
data: {"v":{"message":{"author":{"role":"assistant"},"recipient":"all","content":{"parts":["Riv"]},...}},"c":9}

event: delta
data: {"o":"patch","v":[{"p":"/message/content/parts/0","o":"append","v":"ers are"}, ...]}

event: delta
data: {"v":" among"}

data: {"type":"message_stream_complete", ...}
data: [DONE]
```

- A `{"v":{"message":...}}` event starts a new message. It becomes the target
  of later patches.
- Patches are JSON-patch-like: `p` (path), `o` (`append`, `replace`, `add`,
  `patch` for a batch), `v` (value). A bare `{"v":"text"}` appends to the last
  patched path.
- Hidden `system` messages, `thoughts`, `reasoning_recap` and tool messages
  arrive in the same stream. Tool use shows as an assistant message with a
  `recipient` other than `all`, for example `container.exec` (ChatGPT's
  remote sandbox), `web.run`, `genui.search`.
- The final answer is the assistant `text` message with `recipient: "all"`,
  closed by a patch setting `status: finished_successfully` and
  `end_turn: true`.

## Why not call the API directly from jcode

- Proof of work, Turnstile and the client fingerprint are produced by page
  scripts (`/cdn-cgi/challenge-platform/...`). Reproducing them outside the
  browser means imitating an anti-abuse system, which is brittle and against
  the service's terms.
- Reusing one set of page-minted headers for a second request did work once in
  testing, but the tokens are short-lived and single-purpose.

The page-sends, jcode-rewrites approach keeps every check the site performs
and still gives jcode an exact prompt and a raw, live answer stream.

## Implementation in jcode

- `chatgpt_web_ui.js`: `arm_send` installs the `fetch` hook and stores the
  pending prompt. The hook only rewrites the next `POST /backend-api/f/conversation`
  whose single user message equals the per-turn placeholder, and only once.
  Anything else is refused before reaching the network. `stream` reports the
  decoded state for the turn.
- `submit` refuses to click Send when the composer holds a placeholder but the
  hook is not armed, so a placeholder can never be sent on its own.
- `chatgpt_web.rs`: `poll_network_stream` verifies the request carried the
  exact prompt (UTF-16 length and FNV hash), streams progress, and returns the
  final answer. If arming fails, jcode falls back to pasting the full prompt
  and reading the transcript. `JCODE_CHATGPT_WEB_NETWORK=0` forces that path.
