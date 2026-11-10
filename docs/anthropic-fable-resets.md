# Anthropic Fable earned resets

## Current limitation

`[provider].anthropic_auto_fable_reset` defaults to `false` and is independent of
OpenAI's banked-reset opt-in. The internal
`usage::auto_reset_fable_usage(account_label: Option<String>) -> Result<bool>`
hook currently returns `Ok(false)` even when enabled. **Jcode does not redeem
Fable grants automatically or through its existing session-reset command.**
Use the correct account's Claude Settings > Usage > Limit resets > Reset for free
for manual Fable redemption. Check that you are signed into the intended account.

This limitation is deliberate: a guessed program name or response schema could
spend a different grant, falsely report restored weekly usage, or repeatedly spend
non-refundable grants after an uncertain response. No network request or account
fallback occurs in the unsupported hook, including concurrent/repeated calls.
The existing `juniper_tide` session reset is not a Fable earned reset.

## Primary-source investigation, November 10, 2026

Official article:
<https://support.claude.com/en/articles/17506494-claude-fable-5-2-launch-offers>

The article says a Fable reset restores weekly usage spent on Fable, leaving
other models' weekly usage intact, and refills the five-hour window when present.
Redemption is irreversible and may be done before exhaustion. It documents only
the website Settings > Usage flow, not an OAuth/API endpoint, program identifier,
request/response schema, grant identity, or idempotency contract.

Installed primary implementation inspected without executing a redemption:

- `claude --version`: `2.1.267 (Claude Code)`.
- Resolved executable: `~/.local/share/claude/versions/2.1.267`.
- SHA-256: `0399c793ff571d5946ef923d80b4f330d05ac4b6842a6b0775468f5d389403c0`.
- Static string inspection of the embedded JavaScript finds the existing
  `/api/organizations/${organizationUuid}/reset_rate_limits` POST and its
  `juniper_tide` program. Its results are `reset`, `already_used`, `not_limited`,
  `ineligible`, and `unavailable`. The associated usage block is `juniper_tide`.
- Searches for Fable/reset adjacent references found no Fable redemption
  implementation. This binary includes Fable model and consent code, but that
  does not establish a Fable reset API. This is evidence about this installed
  version, not proof that a newer CLI or private API cannot support resets.
- Searches of official documentation and the Anthropic Claude Code repository
  did not establish an authoritative Fable redemption contract.

No authenticated mutation, grant spend, CLI update, or credential disclosure was
needed for this investigation.

## Requirements before enabling redemption

Obtain an actual primary implementation or documented contract first. Then add:

1. Explicit-account OAuth loading with no active-account fallback and verified
   account/organization identity pinned through preparation and redemption.
2. Fresh exhaustion and eligible unexpired Fable-grant checks, not a generic
   rate-limit error, stale cache, or exhausted unrelated weekly model window.
3. Cross-process locking keyed by verified account identity. Recheck exhaustion
   and grant state under the lock so concurrent agents cannot spend two grants.
4. A durable claim journal written before the mutation and the provider's actual
   idempotency/grant-identity mechanism. A timeout or unrecognized response must
   suppress further uncertain spending until reconciled with authoritative state.
5. A strictly decoded response and fresh evidence that the relevant quota is
   restored before returning `true`, followed by account-specific cache invalidation.
6. Mock-server tests using the real schema for wrong accounts, non-exhaustion,
   stale state, parallel callers, expired grants, uncertain POSTs, restarts, and
   successful redemption. Manual UI wording must distinguish Fable weekly resets
   from five-hour-only session resets and require explicit confirmation.
