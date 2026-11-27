# Provider identity probe: Anthropic and ChatGPT OAuth (2026-11-05)

## Why this exists

On Claude subscription (OAuth) requests, Jcode used to put this sentence near
the top of the system prompt:

> You are a Claude agent, built on Anthropic's Claude Agent SDK.

It was added in `96cdd995b` (2026-04-16, "Fix Claude OAuth third-party request
path"). That commit also replaced the older "You are Claude Code..." sentence
and the "You are jcode, powered by Claude Code..." line, moved the billing tag
from an HTTP header into the system prompt, and bumped its version, all at once.
The commit message doesn't say which of those changes fixed the request path.

Side effect: the model believed the sentence. When a user asked "who are you",
it said Jcode is built on the Claude Agent SDK, which is false. Nothing in the
prompt told it otherwise, because the jcode line had been removed.

## What was tested

Raw requests to `https://api.anthropic.com/v1/messages?beta=true` with the
claude-otter OAuth token. Each request kept Jcode's normal OAuth headers
(Bearer auth, Claude CLI User-Agent, `claude-code-20250219,oauth-2025-04-20`
betas, `x-app: cli`, session id). Only the system blocks changed. Prompt: "Who
are you, in one sentence?"

The billing tag is this first system block:

```
x-anthropic-billing-header: cc_version=2.1.280; cc_entrypoint=sdk-cli; cch=33f85;
```

| Variant | Opus 4.5 (x3) and Sonnet 4.5 | Model's answer |
|---|---|---|
| A. Billing tag + SDK sentence (old current) | works | Sometimes "built on Anthropic's Claude Agent SDK" |
| B. Billing tag, no identity sentence | works | "I'm Jcode..." with no SDK claim |
| C. SDK sentence, no billing tag | works | Claims it's built on the Agent SDK |
| D. Neither | rejected every time, HTTP 429 `rate_limit_error` | n/a |
| E. Billing + SDK + "you are actually Jcode" note | works | mixed |
| F. Old setup (billing + "Claude Code" + jcode note) | works | "I'm Jcode..." |
| G. Billing + jcode prompt, then SDK sentence | works | "I'm Jcode..." |

Haiku 4.5 accepted every variant, including D, so it can't be used to tell the
variants apart. Use Opus or Sonnet.

The 429 on D isn't a real rate limit, because the other variants run right
before and after it succeeded. It looks like Anthropic's way of turning away
OAuth traffic that doesn't look like Claude Code.

## Does the model see the billing tag?

No. Anthropic strips it on the server:

| System blocks | `usage.input_tokens` |
|---|---|
| SDK sentence + short prompt | 34 |
| Billing tag + short prompt | 18 |
| Billing tag + SDK sentence + short prompt | 34 |

The billing tag adds 0 tokens and the SDK sentence adds 16. When asked, Sonnet
also said there was no `cc_version` or `cch` in its context.

## Decision

Send only the billing tag and no identity sentence. Either one is enough to get
accepted, and the billing tag is invisible to the model, so it can't make the
model misdescribe itself.

## Follow-up: the rest of the disguise (same day)

After the identity sentence, every other Claude Code / Codex CLI mimicking
piece was tested the same way and removed when it turned out not to be needed.

### Anthropic (Claude subscription)

Run on Opus 4.5 and Sonnet 4.5, always with the billing tag unless noted:

| Request | Result |
|---|---|
| Full Claude Code headers (CLI User-Agent, `x-app: cli`, `X-Stainless-*`, browser-access, `claude-code-20250219` beta) | works |
| `User-Agent: jcode/...`, none of those headers | works |
| Full headers but only the `oauth-2025-04-20` beta | works |
| Plain UA + only `oauth-2025-04-20` | works |
| Plain UA + only `oauth-2025-04-20`, **no billing tag** | 429 every time |
| Jcode's own tool names (`bash`, `read`, `agentgrep`, ...) instead of `Bash`/`Read` | works, with full or plain headers |
| Realistic request: 4x AGENTS.md prompt, 42 tools, thinking, streaming, plain headers | works, model calls `todo` |
| Same realistic request, no billing tag | 429 |

The plain and full-header requests return identical
`anthropic-ratelimit-unified-*` headers (same 7-day bucket and utilization), so
they bill to the same subscription. Prompt caching works with plain headers
(second call: `cache_read_input_tokens` equals the first call's creation).
`/api/oauth/usage` and `/v1/models` also answer the same with plain headers.

Removed:
- Claude CLI User-Agent, now `jcode/<version>` (`CLAUDE_CLI_USER_AGENT`)
- `x-app: cli`, all `X-Stainless-*`, `X-Claude-Code-Session-Id`,
  `anthropic-dangerous-direct-browser-access` (`apply_oauth_attribution_headers`)
- `claude-code-20250219` beta from every list
- outbound tool renaming to Claude Code names. Inbound mapping still accepts
  the old `Bash`/`Read`/... names so old transcripts and habits still resolve.
- the sidecar's "You are Claude Code..." plus jcode notice, replaced by the
  billing tag

Kept: the billing tag system block, `oauth-2025-04-20`, `?beta=true`, Bearer auth.

### OpenAI (ChatGPT login, Codex backend)

Jcode sends no identity text on this path. The only disguise was the
`originator: codex_cli_rs` header. Tested on chatgpt.com/backend-api/codex with
two ChatGPT accounts on gpt-5.6-sol and gpt-6-astra, over HTTPS and websocket:

| originator | Result |
|---|---|
| `codex_cli_rs` | works |
| `jcode` | works |
| none | works |

All three get identical `x-codex-plan-type`, `x-codex-active-limit` and usage
percentage headers, so the plan accounting doesn't change. The originator is
now `jcode` in the runtime, websocket, compaction and sidecar paths.

(gpt-5.4 and gpt-5-codex returned "not supported when using Codex with a
ChatGPT account" for every originator. That's an account/model limit, not a
header check.)

Not changed: the ChatGPT OAuth *login* URL still passes
`originator=codex_cli_rs` and uses the Codex CLI client id
(`crates/jcode-base/src/auth/oauth.rs`). The login flow is tied to OpenAI's
registered OAuth client and redirect, and testing it means re-logging in, so
it was left alone.

### Other providers

Kimi / Alibaba coding plan / Z.ai still send `User-Agent: claude-cli/1.0.0`
and `x-app: cli` (`jcode-provider-openrouter-runtime/src/request_headers.rs`,
`src/cli/auth_test/choice.rs`). Those coding plans gate on a Claude Code
client. It wasn't tested here because no credentials for them were
available.

## Not covered

- These were short prompts with no tools. A full Jcode session (long prompt,
  tools, thinking) could be checked differently. Check a real session on Opus
  after any change here.
- Anthropic can change these checks at any time. Re-run the probe if OAuth
  requests start getting 429s right away.
- The sidecar (`crates/jcode-base/src/sidecar.rs`) was changed in the
  follow-up below.

## Re-running

```
scripts/anthropic_oauth_identity_probe.py claude-opus-4-5
```

Uses the first Anthropic account in `~/.jcode/auth.json`. Each variant costs
one small request.
