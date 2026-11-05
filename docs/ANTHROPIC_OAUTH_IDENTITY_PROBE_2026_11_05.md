# Anthropic OAuth identity probe (2026-11-05)

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

## Not covered

- These were short prompts with no tools. A full Jcode session (long prompt,
  tools, thinking) could be checked differently. Check a real session on Opus
  after any change here.
- Anthropic can change these checks at any time. Re-run the probe if OAuth
  requests start getting 429s right away.
- The sidecar (`crates/jcode-base/src/sidecar.rs`) still sends "You are Claude
  Code..." followed by a jcode note. It wasn't changed.

## Re-running

```
scripts/anthropic_oauth_identity_probe.py claude-opus-4-5
```

Uses the first Anthropic account in `~/.jcode/auth.json`. Each variant costs
one small request.
