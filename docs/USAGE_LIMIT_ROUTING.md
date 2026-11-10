# Usage exhaustion and temporary throttles

Jcode treats a definitive exhausted usage window differently from a temporary
rate limit. HTTP 429 alone, including Anthropic's “would exceed your account's
rate limit”, does not prove the account has exhausted its plan allowance.

Switching between OpenAI-compatible profiles retains the cancelable countdown
before resending a conversation to another endpoint.

## Configuration

In `~/.jcode/config.toml`:

```toml
[provider]
# Allow switching among explicitly enrolled accounts on the same provider.
same_provider_account_failover = true

# Cross-provider switching still requires an approved auto-switch pool route.
# "manual" disables automatic cross-provider resends entirely.
cross_provider_failover = "countdown"

# Skip the TUI's three-second countdown for definitive usage exhaustion.
# false retains the cancelable countdown even for exhausted usage.
exhausted_usage_immediate_failover = true

# Prefer bounded same-account retries for recoverable Anthropic rate limits.
# false lets the account router rotate immediately when another pool member exists.
anthropic_retry_recoverable_rate_limits = true
```

## Behavior

- Explicit exhausted usage or credit balance errors return to the router without
  sleeping. Opted-in earned-reset recovery remains under its existing settings.
- On Anthropic, ambiguous rate-limit errors retry on the current account for at
  most the existing three total attempts when there is no `Retry-After` hint, or
  its remaining delay is at most five seconds. The server's short retry hint is
  respected. Longer hints yield to the account router when alternatives exist.
- Authentication failures and long reset delays do not take the recoverable
  throttle retry path.
- Once same-provider recovery and account selection are exhausted, the TUI skips
  the cross-provider countdown only for definitive exhausted usage, and only if
  `exhausted_usage_immediate_failover` is enabled. Ambiguous failures retain the
  existing countdown. `cross_provider_failover = "manual"` always takes priority.
- No setting enrolls new accounts, enables API billing, purchases credits, or
  bypasses the auto-switch pool. If no approved route remains, Jcode reports the
  failure instead of switching to an arbitrary provider.

The immediate-exhaustion preference controls the TUI countdown. API clients
that opt into server-side failover retain their existing server-side routing
behavior.
