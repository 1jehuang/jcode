# Account Flows: Observability, Privacy, and Support Diagnostics

Security logging, audit events, metrics, traces, alerting, redaction,
retention, and support diagnostics for the six subscription account flows:
(1) device login, (2) browser approval/denial, (3) account state (`/v1/me`),
(4) checkout, (5) billing portal, (6) webhooks/revocation.

Companion to `docs/dev/ACCOUNT_CONTRACT_CONFORMANCE_TESTS.md`. Client-side
surfaces live in this repo; backend surfaces live in `solosystems-backend`.

## Existing surfaces (grounding)

- Client telemetry pipeline: `crates/jcode-telemetry-core/src/lib.rs`
  (`record_auth_success` @1409, `sanitize_telemetry_label` applied to all
  auth labels @398-400; `AuthEvent` schema in
  `crates/jcode-usage-types/src/lib.rs:270-284` has NO account_id/email slot,
  by design — see the TODO at `src/cli/login/jcode_device.rs:370-374`).
- Onboarding auth funnel: `auth_provider`, `auth_method`,
  `auth_failure_reason` on onboarding step events
  (`crates/jcode-telemetry-core/src/lib.rs:369-400`).
- Failure classification labels: `crates/jcode-base/src/auth/login_diagnostics.rs`
  (`AuthFailureReason::label`) — the only failure detail that may be reported.
- Local logs: `~/.jcode/logs/jcode-YYYY-MM-DD.log`.
- Backend telemetry store: `telemetry-worker/` (D1; subscription analytics
  columns from migration `0016_web_subscription_analytics.sql`).
- Public privacy contract: `TELEMETRY.md` ("anonymous, minimal", no prompts
  or code; opt-out env vars).

## Never-log list (both repos, enforced by tests)

These values must never appear in logs, telemetry, traces, crash reports, or
support bundles, at any log level:

- `api_key` (JCODE_API_KEY) — full or truncated beyond `jc_...last4`.
- `device_code`, magic-link tokens, approval URL query params.
- Raw email addresses in telemetry (local logs may show what the user already
  sees on screen; telemetry must not carry email; backend audit log stores
  email only in the audit table, not app logs).
- Stripe secrets: webhook signing secret, customer payment details, full
  checkout/portal session URLs (they embed session secrets).
- Env-file contents (`jcode-subscription` env file) and `Authorization`
  headers in any HTTP client debug logging.

Enforcement: SN-03 conformance test greps captured login output; add a
CI grep over `record_*` call sites asserting only sanitized labels flow into
telemetry (all auth labels already pass through `sanitize_telemetry_label`).

## Audit events (backend, durable)

Append-only audit table keyed by `account_id`, retained 400 days:

| Event | Required fields |
|---|---|
| `device_auth.requested` | account_id?, email_hash, ip_hash, user_agent class, device_code_id (opaque id, not the code) |
| `device_auth.approved` / `denied` | device_code_id, approver session id, reason |
| `device_auth.expired` | device_code_id |
| `key.issued` | key_id (not the key), tier |
| `key.revoked` | key_id, actor (user/portal/admin/system), reason |
| `checkout.started` / `completed` / `abandoned` | stripe session id, tier target |
| `portal.opened`, `subscription.updated` / `canceled` | stripe ids, old->new tier/status |
| `webhook.received` / `applied` / `rejected` | stripe event id, type, signature result, dedup outcome |
| `me.tier_changed` | old, new, cause (webhook/admin) |

Correlation requirements: every audit row carries `account_id`,
`request_id` (per HTTP request), and `flow_id` (one device-login attempt or
one checkout attempt). `device_code_id` links flows 1-2; `stripe_event_id`
links 4-6. The client sends no correlation IDs today; if added, use a random
per-attempt UUID only, never the telemetry install id joined with account_id
in the same event (keeps anonymous telemetry unlinkable to accounts).

## Client telemetry (this repo, anonymous)

Keep the current shape: success events with coarse `auth_provider` +
`auth_method` ("jcode-subscription", "device_code_magic_link"), failure
reasons restricted to `AuthFailureReason::label` values. Additions:

- `auth_failure` event mirroring `auth_success` (reason label only).
- Poll-loop outcome counter: approved/denied/expired/slow_down/error, plus
  bucketed time-to-approve (e.g. <30s, <2m, <15m) — no timestamps precise
  enough to correlate with backend logs.
- Do NOT add `account_id` to `AuthEvent` (the existing TODO at
  `src/cli/login/jcode_device.rs:370` should be resolved as "backend audit
  owns account-linked events; client telemetry stays anonymous").

## Metrics and alerting (backend)

Metrics (per flow, labeled by outcome and tier where applicable):

- `device_auth_requests_total`, `device_auth_approvals_total`,
  `device_auth_denials_total`, `device_auth_expiries_total`;
  alert: approval rate < 50% over 1h, or expiry rate > 40% (email delivery
  problem), or request spike > 10x baseline (enumeration/abuse).
- `token_poll_requests_total{result}`; alert on `slow_down` ratio > 20%
  (client misbehavior or attack) and on unexpected-5xx ratio > 1%.
- `me_requests_total{status}`; alert on 401 spike (mass revocation bug) and
  p99 latency > 2s (client timeout is 5s: `ME_FETCH_TIMEOUT`,
  `crates/jcode-base/src/subscription_api.rs:14`).
- `webhook_events_total{type, outcome=applied|duplicate|rejected}` and
  `webhook_apply_lag_seconds` (Stripe `created` -> state applied);
  alert: rejected signatures > 0 sustained, lag p95 > 60s (this is the
  RV-01/CK-02 conformance bound), any DLQ depth > 0 for 15m.
- `key_revocations_total{actor}`; alert on system/admin bulk revocations.

Client-side (via existing anonymous telemetry, dashboarded in
telemetry-worker): login success ratio by version, `auth_failure_reason`
distribution — regression alert when a new release shifts failure mix.

## Traces

Backend: one trace per request; parent span per flow_id so a device-login
attempt shows request -> email send -> approval -> token issue as linked
spans. Span attributes limited to the audit-field set (ids and hashes, never
secrets/emails). Stripe webhook handling gets a span per event with
`stripe_event_id` and dedup decision.

Client: no distributed tracing for auth (privacy). Local log lines around the
login flow use the daily log with the flow outcome only.

## Retention

| Data | Where | Retention |
|---|---|---|
| Audit events | backend | 400 days, append-only |
| Backend app logs | backend | 30 days |
| Traces | backend | 7-14 days |
| Stripe webhook payload archive | backend (encrypted) | 90 days |
| Anonymous client telemetry | telemetry-worker D1 | per TELEMETRY.md; no account linkage |
| Local client logs | `~/.jcode/logs/` | user-owned; must satisfy never-log list |
| Support bundles | created on demand | delete after case close (<= 90 days) |

## Support diagnostics

`jcode doctor`-style account diagnostics (extend existing provider doctor,
`crates/jcode-provider-doctor/`): prints auth_base, masked key
(`jc_...last4`), account email as stored locally, cached tier
(`subscription_catalog::cached_tier`), last `/v1/me` status, env-file path
and permissions. Copy-safe by construction (never full key). Support asks
the user for: masked key id, approximate login time, and the
`AuthFailureReason` label; backend support joins on `key_id`/`email_hash` in
the audit table. No flow requires the user to paste a key or link.

## Per-flow one-page summary

1. Device login: client logs outcome label only; backend audits request/
   approve/expire with device_code_id; metric+alert on approval/expiry rates.
2. Browser approval/denial: backend-only; audit approver session, alert on
   denial spikes; page must not log the magic token (only its hash).
3. `/v1/me`: backend request logs with account_id + request_id; client caches
   tier and logs nothing sensitive; alert on 401 spikes and latency.
4. Checkout: audit started/completed/abandoned via Stripe ids; funnel metric;
   never log payment details (Stripe owns them).
5. Portal: audit opened + resulting subscription changes; portal URLs are
   secrets (never logged).
6. Webhooks/revocation: audit every event with signature + dedup outcome;
   lag and DLQ alerts; revocation audit rows are the support source of truth.

## Follow-ups (actionable in this repo)

- Add `record_auth_failure` for the jcode device flow (labels only).
- Resolve the `account_linked` TODO in `jcode_device.rs` as "won't add to
  client telemetry" and point to backend audit.
- Add SN-03-style output-capture test asserting the never-log list for the
  login flow, and a doctor command for account diagnostics.
