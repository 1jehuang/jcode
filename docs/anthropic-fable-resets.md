# Anthropic Fable earned resets

Set `[provider].anthropic_auto_fable_reset = true` to opt in. The default is
`false`, independent of OpenAI's banked-reset setting. Automatic retry integrates
through `usage::auto_reset_fable_usage(account_label) -> Result<bool>`.

This redeems earned Fable grants, never buys usage credits, and never substitutes
the separate `juniper_tide` five-hour reset program. `true` requires both a
successful reset response and fresh usage showing no exhausted limit, or a
fresh clear status after waiting for another agent's account lock. Missing,
malformed, expired, paused, blocked or unrelated grants fail closed.

Automatic redemption currently requires a named stored OAuth account. `None`
returns false rather than risking a changing active-account fallback. The exact
account token is pinned for profile, status and claim. Account and organization
UUIDs from its profile scope the cross-process lock, including account aliases.

## Verified primary contract, November 10, 2026

Offer semantics:
<https://support.claude.com/en/articles/17506494-claude-fable-5-2-launch-offers>

A Fable reset restores weekly usage spent on Fable, leaving other models' weekly
usage intact, and refills the five-hour window when present. Redemption is
irreversible. The website permits early redemption, but Jcode automatic resets
require actual server-confirmed weekly exhaustion and a usable grant. A
five-hour-only wall never spends a Fable grant automatically.

The installed Claude Code **2.1.267** contains only the older `juniper_tide`
contract. The newer official npm release **2.1.322** establishes the actual Fable
contract. It was downloaded and inspected statically, not installed or executed:

- Metadata: <https://registry.npmjs.org/@anthropic-ai/claude-code/latest>
- Platform metadata: <https://registry.npmjs.org/@anthropic-ai/claude-code-linux-x64/2.1.322>
- Official package tarball: <https://registry.npmjs.org/@anthropic-ai/claude-code-linux-x64/-/claude-code-linux-x64-2.1.322.tgz>
- npm platform tarball SHA-1: `b56040e7f01b956839d56504882d36a177f06a89`.
- Read-only GET: `/api/oauth/usage?cedar_ember=1&skip_spend=1`.
- Status block: `cedar_ember`, containing `eligible`, `at_limit`, `exhausted`,
  `grants`, `next_grant_id`, `weekly_resets_at`, `cooldown_until`.
- Grant fields include `id`, `resets_left`, `starts_at`, `ends_at`, `clears`,
  `paused`, `usable_now`, `use_requires_limit`, `percent_used`, `blocking`,
  `type`, `gives_back_weekly`, `unavailable_reason`.
- The CLI's `Ye` classifier identifies Fable grants by `gives_back_weekly`.
- POST: `/api/organizations/{organizationUuid}/reset_rate_limits`, JSON
  `{"program":"cedar_ember","grant_id":"<grant>","request_id":"<uuid>"}`.
- Claim results: `reset`, `already_used`, `not_limited`, `cooldown`, `ineligible`,
  `unavailable`, with `reason`, `resets_left`, `cleared`, `weekly_resets_at`,
  `cooldown_until`. An uncertain claim is not permission to spend another grant.
- Grant IDs match `^[a-z0-9_-]{1,40}$`. Request IDs match
  `^[A-Za-z0-9_-]{1,64}$`. Jcode uses a UUID request ID.

Public Settings bundles were investigated too. Curl of
<https://claude.ai/settings/usage> returned a Cloudflare challenge. The browser
opened the app and exposed the public script
`https://assets-proxy.anthropic.com/claude-ai/v2/assets/v1/index-B7IpSks2.js`.
That bundle and the loaded public JavaScript resource graph were searched for
`cedar_ember`, `reset_rate_limits`, and `gives_back_weekly`, without finding a
matching implementation. The implementation source of truth is therefore the
updated official CLI, not guessed website endpoints. No authenticated mutation
or real grant spend was used for discovery or tests.

## Concurrency and uncertain outcomes

Jcode uses an OS file lock per verified account/organization identity and checks
fresh status under that lock. Only exhausted all-model weekly or Fable
included-weekly limits that the grant can address qualify. A simultaneous
five-hour wall must also be cleared by the grant. A server-blocked
or unrelated model-specific limit never triggers spending.

Before POST, Jcode creates and fsyncs a durable journal at
`~/.jcode/fable-resets/<identity-hash>.pending` (or the configured Jcode data
root). It stores program, grant ID, request ID and time, never tokens or email.
The journal remains after HTTP errors, timeouts, cancellation, process death,
unknown responses or an unconfirmed reset. Subsequent calls, including after a
restart, cannot spend another grant. This is deliberately more conservative
than automatically replaying the same request ID.

Only a confirmed reset plus fresh clear quota removes the journal. If a journal
remains, inspect the intended account's Claude Settings > Usage and reconcile
the recorded grant before manually removing that account's `.pending` file.
Do not remove it just to retry an uncertain request. Lock files are persistent
coordination objects and should not be deleted while agents run.

Existing manual session-reset UI still refers only to `juniper_tide`. For manual
Fable redemption, use the intended account's Claude Settings > Usage > Limit
resets > Reset for free. The Fable automatic setting does not alter manual UI or
provide permission to spend early.
