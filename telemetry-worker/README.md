# jcode Telemetry Worker

Cloudflare Worker that receives anonymous telemetry events from jcode.

The headline number is **Total users**: distinct, non-CI `telemetry_id`s that
ever installed jcode OR did meaningful work in it. Run it with:

```bash
wrangler d1 execute jcode-telemetry --remote --file=users.sql
```

## Storage architecture

Events are dual-written to two stores with different jobs:

1. **Workers Analytics Engine firehose** (`jcode_telemetry_firehose` dataset):
   every event, written first. Time-series store with no database size cap and
   ~90-day retention (adaptive sampling on reads; `index1` is the
   `telemetry_id`, so per-user sampling stays accurate). This is the primary
   store for high-volume raw analysis (`turn_end`, `session_start`,
   `onboarding_step` volume) and the safety net: telemetry keeps recording
   even when D1 is full. Column mapping lives in `FIREHOSE_SCHEMA` in
   `src/worker.js` and is **append-only** (never reorder or repurpose a
   position). Query it via the [Analytics Engine SQL API](https://developers.cloudflare.com/analytics/analytics-engine/sql-api/):
   ```bash
   # Requires an API token with Account Analytics read. Example: auth failure
   # reasons over the last 7 days (blob9=auth_provider, blob11=auth_failure_reason).
   curl -s "https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/analytics_engine/sql" \
     -H "Authorization: Bearer $CF_ANALYTICS_TOKEN" \
     -d "SELECT blob9 AS provider, blob11 AS reason, SUM(_sample_interval) AS n
         FROM jcode_telemetry_firehose
         WHERE blob1 = 'onboarding_step' AND blob8 = 'auth_failed'
           AND timestamp > NOW() - INTERVAL '7' DAY
         GROUP BY provider, reason ORDER BY n DESC"
   ```
2. **D1** (`jcode-telemetry` database): the durable relational store for
   identity anchors (`install`, `feedback`), auth/lifecycle events, the
   `daily_active_users` rollup, and a retention-pruned raw tail of the
   high-volume events (see `RETENTION_DAYS`). All the dashboard SQL in this
   repo (`users.sql`, `dau.sql`, `geo.sql`, `health.sql`) reads D1.

Separately consented full transcripts do not enter either firehose or the
ordinary `events` table. `POST /v1/transcript` writes the JSON body to the
private `TRANSCRIPTS` R2 bucket and writes metadata to `transcript_uploads`.
Create the bucket before deployment and configure a 30-day lifecycle deletion:

```bash
npx wrangler r2 bucket create jcode-consented-transcripts
npm run migrate:transcript-uploads
```

The bucket must remain private. Deployment alone does not create the lifecycle
rule; configure it in Cloudflare before enabling the program in a release.

### Transcript access and deletion operations

Treat transcript access as a production-data operation. Do not expose the R2
bucket publicly, copy transcript bodies into logs, or query them from ordinary
analytics dashboards. Use an account with narrowly scoped R2 read access and
record the reason and upload ID for every manual read.

To remove one upload, first look up its private object key, delete the R2 object,
then delete the metadata row. Verify both stores no longer contain it:

```bash
npx wrangler d1 execute jcode-telemetry --remote --command \
  "SELECT object_key FROM transcript_uploads WHERE upload_id='<UPLOAD_ID>'"
npx wrangler r2 object delete \
  "jcode-consented-transcripts/<OBJECT_KEY>" --remote
npx wrangler d1 execute jcode-telemetry --remote --command \
  "DELETE FROM transcript_uploads WHERE upload_id='<UPLOAD_ID>'"
```

For deletion by installation telemetry ID, enumerate every `upload_id` and
`object_key` first, delete every R2 object, then delete the matching D1 rows.
Never delete the metadata first because that loses the keys needed to locate
the private objects. The 30-day R2 lifecycle is the backstop, not a substitute
for explicit deletion requests.

### D1 size self-defense

D1 hard-caps databases at 10 GB on Workers Paid (500 MB on Free). The first
5 GB of account-wide paid storage is included. The worker therefore uses a
4.5 GB soft limit, leaving room for other databases and for pruning to catch
up before the 10 GB hard cap. At the old free-plan cap every insert failed and
telemetry silently stopped (June 2026: ~3 days lost). Defenses, in order:

- The worker observes `meta.size_after` on every D1 write. Past the soft
  budget limit (`D1_SOFT_LIMIT_BYTES`) it
  triggers an **emergency prune** (halved retention windows, rate-limited to
  one per 10 minutes per isolate) instead of waiting for the nightly cron.
- If an insert fails with a SQLITE_FULL-class error, the emergency prune runs
  immediately, bounding a June-style outage to minutes instead of days.
- The nightly cron re-checks size after the normal prune and escalates to the
  emergency prune if still over the soft limit.
- If a D1 insert still fails, the request returns `{ok, durable:false,
  firehose:true}` instead of a 500, because the event was captured in the
  firehose.
- `GET /v1/health` reports `db_size_bytes` vs the soft limit for external
  monitoring.

Note: D1 has no `VACUUM`, so the file never shrinks; deletes only free pages
internally for reuse. If bloat itself becomes the problem, rotate to a fresh
database (create new D1 DB, copy live rows, repoint `wrangler.toml`).

## Setup

1. Install wrangler: `npm install`

2. Create D1 database:
   ```bash
   wrangler d1 create jcode-telemetry
   ```

3. Update `wrangler.toml` with the database ID from step 2

4. Initialize schema:
   ```bash
   wrangler d1 execute jcode-telemetry --file=schema.sql
   ```

### Migrating an existing database

If your production database was created before the latest telemetry fields were added,
apply all remote migrations:

```bash
wrangler d1 execute jcode-telemetry --remote --file=migrations/0001_expand_events.sql
wrangler d1 execute jcode-telemetry --remote --file=migrations/0002_transport_metrics.sql
wrangler d1 execute jcode-telemetry --remote --file=migrations/0003_usage_expansion.sql
wrangler d1 execute jcode-telemetry --remote --file=migrations/0004_telemetry_phase123.sql
wrangler d1 execute jcode-telemetry --remote --file=migrations/0005_workflow_turn_telemetry.sql
```

(...and so on through the latest numbered migration; each also has an
`npm run migrate:<name>` alias, see Ops helpers below. The newest is
`migrations/0018_web_quality_telemetry.sql` / `npm run migrate:web-quality`.)

Then redeploy the worker:

```bash
npm run deploy
```

5. Deploy:
   ```bash
   npm run deploy
   ```

6. Set up custom domain (optional): point `telemetry.jcode.dev` to the worker in Cloudflare dashboard

### Ops helpers

```bash
# Apply schema catch-up migrations
npm run migrate:expand
npm run migrate:transport
npm run migrate:usage
npm run migrate:phase123
npm run migrate:workflow
npm run migrate:tokens
npm run migrate:dashboard-indexes
npm run migrate:feedback-text
npm run migrate:daily-active
npm run migrate:daily-active-backfill
npm run migrate:daily-active-ci
npm run migrate:detail-fields
npm run migrate:dau-full-backfill
npm run migrate:auth-failure-reason
npm run migrate:web-subscription
npm run migrate:discovery
npm run migrate:web-quality
npm run migrate:model-prices

# Run a dashboard query. These go through scripts/run-dashboard.mjs, which
# sends the file via `--command` instead of `--file`: wrangler's `--file` path
# is D1's *import* API and prints only "Rows read / Rows written / Database
# size", discarding the result set, so these panels used to render no data.
npm run health
npm run dau
npm run users
npm run token-value
```

## Token value dashboard

`npm run token-value` reports the list-price dollar value of the token flow
through jcode, priced per model rather than with one blended rate. Setup:

```bash
npm run migrate:model-prices   # creates model_prices (migration 0023)
npm run sync:model-prices      # fills it from https://models.dev/api.json
npm run token-value            # daily / per-model / summary panels
npm run token-value:daily      # just the per-day series, in date order
```

`npm run token-value:daily` is the plain time series when all you want is
"dollars per day": one row per day with the tokens, sessions, and distinct
users behind it. There is deliberately no per-user dollar column, because it
tracked tokens-per-user almost exactly (coefficient of variation 0.147 vs
0.142 over a 10-day sample): the blended rate per million tokens barely moves,
so it was the same series twice in different units.

`scripts/sync-model-prices.mjs` reads the model labels actually observed in
telemetry (`events.model_end` on `session_end` rows) and matches each one to a
models.dev price, normalizing the gateway aliases users produce
(`cc/claude-opus-5`, `openai/gpt-5.6-sol`, `claude-opus-4-5-20251101`,
`...-4-8@Anthropic`, `-xhigh` effort suffixes). Re-run it after new models
appear; it is an idempotent upsert. Current token coverage is ~97%, with the
remainder being users' private gateway aliases (`my-coding`, `SeaaveyCombo`)
that cannot be resolved to a public price.

Three things to know before quoting the number:

- **Cache accounting is provider-specific.** OpenAI-compatible APIs report
  cached tokens as a *subset* of prompt tokens; Anthropic reports them as a
  disjoint bucket. `model_prices.input_includes_cache_read` drives the
  correction. Skipping it overcharges OpenAI traffic ~10x, and since cache
  reads are ~85% of all tokens, that error dominates the total.
- **It is list price, not spend.** Most traffic runs on subscriptions (Claude
  Max, ChatGPT Pro, Copilot) or free routes, so read it as "list-price
  equivalent value of tokens served".
- **Check `priced_token_pct` / `unpriced_tokens`.** Every panel reports them.
  If coverage drops, re-run the sync before trusting the dollar figure.


## Reading DAU without fooling yourself

`npm run dau` leads with `headline_users_24h` (= `meaningful_release_24h_noci`):
real users, release channel, CI excluded. Use that number.

Two traps the panel now guards against:

- **Partial day.** The `today` tiers cover a partial UTC day, so every morning
  they look like a cliff. `day_elapsed_pct` plus `release_users_sofar` /
  `..._yday` / `..._7d` compare today against the *same clock window* on prior
  days, and `pace_vs_yday` / `pace_vs_7d` are the ratios (>1.0 = ahead). These
  are same-window comparisons, not extrapolations, because DAU is a distinct
  count and does not scale linearly with elapsed time.
- **Dev-build traffic.** `debug` and `git_checkout` ids are overwhelmingly
  throwaway: a `session_start` and an `onboarding_step`, no `session_end`
  (7-day completion ratio 0.02 for `debug` vs 0.21 for `release`). Their volume
  swings ~5x day to day, which is enough to make a flat week look like
  alternating spikes and cliffs in any raw-id metric. `dev_build_24h` tracks
  them so the swing is visible instead of silently moving the headline.

This is also why the overall `lifecycle_completion_ratio` in `health.sql` is
low: it is a blend across channels, and the dev channels drag it down.

Release's own ratio was ~0.25 for a separate reason: `begin_session` replaced
a live in-process session without ending it, so every superseded session's
`session_start` was orphaned. Those now emit a `session_end` with
`session_stop_reason = 'superseded'`. Expect the release ratio to climb as
clients upgrade, and expect `superseded` to be a large share of ends: it means
one process opened several sessions, not that anything failed.

## Event types

CLI events (sent by jcode itself): `install`, `upgrade`, `auth_success`,
`onboarding_step`, `feedback`, `session_start`, `turn_end`, `session_end`,
`session_crash`.

### Website analytics and quality events (migrations 0016 and 0018)

Sent by the beacon on `https://jcode.sh` (and the
`https://solosystems.pages.dev` preview). The browser mints an anonymous
`visitor_id` UUID in localStorage; the worker uses it as the telemetry id and
fills in `version`/`os`/`arch` defaults, so the beacon payload can stay tiny.
Web-only fields are stored in the `web_details` table (keyed by `event_id`,
like `session_details`/`turn_details`) because `events` is near D1's
100-column cap.

- `web_pageview`: `path`, `referrer`, `visitor_id`, `utm_source`,
  `utm_medium`, `utm_campaign`
- `web_cta_click`: `path`, `cta` (e.g. `plus_early_access`,
  `flagship_early_access`, `install`), `visitor_id`, `session_id`, `pageview_id`.
  Install actions also carry a per-click random `conversion_id`, CTA `placement`, and
  `install_method`.
- `web_vital`: `path`, `visitor_id`, standard `metric_name` (`CLS`, `FCP`,
  `INP`, `LCP`, or `TTFB`), finite nonnegative `metric_value`, and `rating`
  (`good`, `needs-improvement`, or `poor`). Values are capped at 10 for CLS
  and 300000 ms for the other metrics. D1 retention is 30 days.
- `web_error`: `path`, `visitor_id`, and coarse `error_kind` (`script`,
  `promise`, or `resource`). Error messages, stacks, filenames, and URLs are
  never stored. D1 retention is 90 days.

### Install conversion funnel (migration 0020)

The website mints an opaque UUID only when an install/download CTA is used and
browser DNT/GPC is not active. For the shell path, the same UUID is carried by
the copied `/install?conversion_id=...` command, injected into the installer,
persisted locally with mode 0600, and attached to the existing post-install
`install` event. The CLI consumes it after a successful send and deletes any
unconsumed token older than 90 days. It is not an account or telemetry ID and
contains no browsing content.

`install_funnel` stages are `command_copy`, `script_request`,
`installer_start`, and `installer_finish`, with `success`/`failure` outcomes.
The existing `install` event receives an `install_details` row with stage
`first_run`. Installer failures retain only a coarse stage label, never command
output, paths, hostnames, or error messages. The scheduled retention job nulls
the cross-system `conversion_id` in both web and install details after 90 days.

Run `npm run conversion` for visitor/session-to-intent rates, copy, script retrieval,
installer success, first-run conversion, stage latency, campaign/path/placement
splits, platform success rates, and coarse platform-specific failures. Direct
binary downloads are measurable through intent only; shell installs can be joined
through first launch. Browser privacy opt-outs and un-attributed/manual installs
are intentionally excluded from the joined funnel. Every conversion-bearing
event is also written to the dedicated
`jcode_install_firehose` Analytics Engine dataset before D1, so the 90-day
funnel is reconstructable during a D1 outage.

### Token subscription plan events (migration 0016)

All require `account_id`; `tier` and `model` are attached where relevant
(`model` is stored in the existing generic `model_start` column).

- `subscription_login`: `account_id`, `tier`
- `subscription_activated`: `account_id`, `tier`
- `subscription_budget_exhausted`: `account_id`, `tier`, `model`
- `subscription_router_error`: `account_id`, `tier`, `model`
- `account_linked`: `telemetry_id` (the standard `id` field) + `account_id`.
  This is the analytics<->account join anchor: it ties an anonymous CLI
  `telemetry_id` to a subscription `account_id`, and is never pruned.

Web + subscription events are firehosed to the separate `jcode_web_firehose`
dataset (`FIREHOSE_WEB_SCHEMA` in `src/worker.js`, also append-only): the
main `FIREHOSE_SCHEMA` is at Analytics Engine's 20-blob/20-double capacity.
For web events `index1` is the `visitor_id`.
The 0018 fields were appended without reordering: `blob18=metric_name`,
`blob19=rating`, `blob20=error_kind`, and `double2=metric_value`.

## Querying Data

```bash
# Where are our users? (country only; see migration 0022 and TELEMETRY.md)
npm run geo   # or: wrangler d1 execute jcode-telemetry --remote --file=geo.sql

# Users by country over the last 30 days, straight from the rollup
wrangler d1 execute jcode-telemetry --remote --command "SELECT COALESCE(last_country, 'unknown') AS country, COUNT(DISTINCT telemetry_id) AS users FROM daily_active_users WHERE activity_date >= date('now', '-30 days') AND last_is_ci = 0 GROUP BY 1 ORDER BY users DESC LIMIT 25"

# Total installs (raw, and excluding CI runners which mint a fresh id per job)
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) AS raw_installs, COUNT(DISTINCT CASE WHEN is_ci = 0 THEN telemetry_id END) AS installs_noci FROM events WHERE event = 'install'"

# Web vitals by route and rating over the retained 30-day D1 window
wrangler d1 execute jcode-telemetry --command "SELECT w.path, w.metric_name, w.rating, COUNT(*) AS samples, AVG(w.metric_value) AS avg_value FROM events e JOIN web_details w USING (event_id) WHERE e.event = 'web_vital' AND e.created_at > datetime('now', '-30 days') GROUP BY 1, 2, 3 ORDER BY 1, 2, 3"

# Classified web errors by route over the retained 90-day D1 window
wrangler d1 execute jcode-telemetry --command "SELECT w.path, w.error_kind, COUNT(*) AS errors FROM events e JOIN web_details w USING (event_id) WHERE e.event = 'web_error' AND e.created_at > datetime('now', '-90 days') GROUP BY 1, 2 ORDER BY errors DESC"

# Analytics Engine web-vital sample counts (append-only positions from 0018)
curl -s "https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/analytics_engine/sql" \
  -H "Authorization: Bearer $CF_ANALYTICS_TOKEN" \
  -d "SELECT blob18 AS metric_name, blob19 AS rating, SUM(_sample_interval) AS samples, AVG(double2) AS avg_value FROM jcode_web_firehose WHERE blob1 = 'web_vital' AND timestamp > NOW() - INTERVAL '7' DAY GROUP BY metric_name, rating ORDER BY metric_name, rating"

# Weekly / monthly active users (canonical: use the rollup so every window
# shares one "meaningful" definition and includes session_crash + turn_end days).
# meaningful_release_*_noci is the headline product metric: real users on the
# release channel, excluding automated CI traffic (ephemeral runners that mint a
# fresh telemetry_id per job and otherwise inflate users/installs and tank retention).
# WAU (last 7 UTC days):
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) AS raw_wau, COUNT(DISTINCT CASE WHEN meaningful_active > 0 THEN telemetry_id END) AS meaningful_wau, COUNT(DISTINCT CASE WHEN meaningful_release_active > 0 THEN telemetry_id END) AS meaningful_release_wau, COUNT(DISTINCT CASE WHEN meaningful_release_active > 0 AND last_is_ci = 0 THEN telemetry_id END) AS meaningful_release_wau_noci FROM daily_active_users WHERE activity_date > date('now', '-7 days')"

# MAU (last 30 UTC days):
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) AS raw_mau, COUNT(DISTINCT CASE WHEN meaningful_active > 0 THEN telemetry_id END) AS meaningful_mau, COUNT(DISTINCT CASE WHEN meaningful_release_active > 0 THEN telemetry_id END) AS meaningful_release_mau, COUNT(DISTINCT CASE WHEN meaningful_release_active > 0 AND last_is_ci = 0 THEN telemetry_id END) AS meaningful_release_mau_noci FROM daily_active_users WHERE activity_date > date('now', '-30 days')"

# Raw vs meaningful active users this week, directly from raw events (matches the
# rollup definition: counts session_end/session_crash AND turn_end activity).
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) AS raw_wau, COUNT(DISTINCT CASE WHEN (event IN ('session_end','session_crash') AND (turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0 OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0 OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0 OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0 OR provider_switches > 0 OR model_switches > 0)) OR (event = 'turn_end' AND (assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0 OR file_write_calls > 0 OR tests_run > 0 OR turn_success > 0)) THEN telemetry_id END) AS meaningful_wau FROM events WHERE event IN ('session_end','session_crash','turn_end') AND created_at > datetime('now', '-7 days')"

# Provider distribution for meaningful sessions
wrangler d1 execute jcode-telemetry --command "SELECT provider_end, COUNT(*) as sessions FROM events WHERE event = 'session_end' AND (turns > 0 OR duration_mins > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0 OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0 OR provider_switches > 0 OR model_switches > 0) GROUP BY provider_end ORDER BY sessions DESC"

# Average meaningful session duration
wrangler d1 execute jcode-telemetry --command "SELECT AVG(duration_mins) as avg_mins, AVG(turns) as avg_turns FROM events WHERE event = 'session_end' AND (turns > 0 OR duration_mins > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0 OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0 OR provider_switches > 0 OR model_switches > 0)"

# Error rates. Count affected sessions/users, not raw sums: raw sums are
# dominated by runaway retry loops (one pre-breaker session logged 18k+ auth
# failures), which makes one broken install look like a fleet-wide outage.
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(CASE WHEN error_provider_timeout > 0 THEN 1 END) as timeout_sessions, COUNT(CASE WHEN error_rate_limited > 0 THEN 1 END) as rate_limited_sessions, COUNT(CASE WHEN error_auth_failed > 0 THEN 1 END) as auth_failed_sessions, COUNT(DISTINCT CASE WHEN error_auth_failed > 0 THEN telemetry_id END) as auth_failed_users FROM events WHERE event = 'session_end'"

# Auth failure reasons (requires 0015; reasons recorded from explicit auth_failed onboarding steps)
wrangler d1 execute jcode-telemetry --command "SELECT auth_provider, auth_failure_reason, COUNT(*) AS n, COUNT(DISTINCT telemetry_id) AS users FROM events WHERE event = 'onboarding_step' AND step = 'auth_failed' AND created_at > datetime('now', '-30 days') GROUP BY 1, 2 ORDER BY n DESC"

# Version adoption
wrangler d1 execute jcode-telemetry --command "SELECT version, COUNT(DISTINCT telemetry_id) as users FROM events GROUP BY version ORDER BY version DESC"

# Heavy telemetry IDs (useful for spotting dev/test noise)
wrangler d1 execute jcode-telemetry --command "SELECT telemetry_id, COUNT(*) AS session_ends FROM events WHERE event = 'session_end' GROUP BY telemetry_id ORDER BY session_ends DESC LIMIT 20"

# OS/arch breakdown
wrangler d1 execute jcode-telemetry --command "SELECT os, arch, COUNT(DISTINCT telemetry_id) as users FROM events GROUP BY os, arch ORDER BY users DESC"

# Transport breakdown (requires 0002 transport migration)
wrangler d1 execute jcode-telemetry --command "SELECT SUM(transport_https) AS https, SUM(transport_persistent_ws_fresh) AS ws_fresh, SUM(transport_persistent_ws_reuse) AS ws_reuse, SUM(transport_cli_subprocess) AS cli, SUM(transport_native_http2) AS native_http2, SUM(transport_other) AS other FROM events WHERE event IN ('session_end', 'session_crash')"

# Telemetry health dashboard
wrangler d1 execute jcode-telemetry --file=health.sql

# Daily active users. Prefer meaningful_release_* as the headline product metric.
npm run dau

# Fast UTC-day DAU from the ingest-time rollup table
wrangler d1 execute jcode-telemetry --remote --command "SELECT COUNT(*) AS raw_today, SUM(CASE WHEN meaningful_active > 0 THEN 1 ELSE 0 END) AS meaningful_today, SUM(CASE WHEN release_active > 0 THEN 1 ELSE 0 END) AS raw_release_today, SUM(CASE WHEN meaningful_release_active > 0 THEN 1 ELSE 0 END) AS meaningful_release_today FROM daily_active_users WHERE activity_date = date('now')"

# Auth activation funnel by provider
wrangler d1 execute jcode-telemetry --command "SELECT auth_provider, COUNT(DISTINCT telemetry_id) AS users FROM events WHERE event = 'auth_success' GROUP BY auth_provider ORDER BY users DESC"

# Onboarding funnel steps
wrangler d1 execute jcode-telemetry --command "SELECT step, COUNT(DISTINCT telemetry_id) AS users FROM events WHERE event = 'onboarding_step' GROUP BY step ORDER BY users DESC"

# Recent explicit feedback
wrangler d1 execute jcode-telemetry --command "SELECT created_at, feedback_text, feedback_rating, feedback_reason, version, build_channel FROM events WHERE event = 'feedback' ORDER BY created_at DESC LIMIT 50"

# Session starts by UTC hour (workflow timing)
wrangler d1 execute jcode-telemetry --command "SELECT session_start_hour_utc, COUNT(*) AS sessions FROM events WHERE event = 'session_start' GROUP BY session_start_hour_utc ORDER BY session_start_hour_utc"

# Multi-sessioning rate
wrangler d1 execute jcode-telemetry --command "SELECT AVG(CASE WHEN multi_sessioned > 0 THEN 1.0 ELSE 0.0 END) AS multi_session_rate FROM events WHERE event IN ('session_end', 'session_crash') AND created_at > datetime('now', '-30 days')"

# Per-turn latency and success
wrangler d1 execute jcode-telemetry --command "SELECT AVG(turn_active_duration_ms) AS avg_turn_ms, AVG(CASE WHEN turn_success > 0 THEN 1.0 ELSE 0.0 END) AS turn_success_rate FROM events WHERE event = 'turn_end' AND created_at > datetime('now', '-30 days')"

# Build-channel cleanup for active users
wrangler d1 execute jcode-telemetry --command "SELECT build_channel, COUNT(DISTINCT telemetry_id) AS users FROM events WHERE event IN ('session_end', 'session_crash') AND created_at > datetime('now', '-30 days') GROUP BY build_channel ORDER BY users DESC"

# D7 retention for users who installed 8-14 days ago
wrangler d1 execute jcode-telemetry --command "WITH cohort AS (SELECT DISTINCT telemetry_id FROM events WHERE event = 'install' AND created_at >= datetime('now', '-14 days') AND created_at < datetime('now', '-7 days')), retained AS (SELECT DISTINCT telemetry_id FROM events WHERE event IN ('session_end', 'session_crash') AND created_at >= datetime('now', '-7 days')) SELECT COUNT(*) AS cohort_users, (SELECT COUNT(*) FROM cohort WHERE telemetry_id IN retained) AS retained_users FROM cohort"

# Feature adoption (last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT SUM(feature_memory_used) AS memory_sessions, SUM(feature_swarm_used) AS swarm_sessions, SUM(feature_web_used) AS web_sessions, SUM(feature_email_used) AS email_sessions, SUM(feature_mcp_used) AS mcp_sessions, SUM(feature_side_panel_used) AS side_panel_sessions, SUM(feature_goal_used) AS goal_sessions, SUM(feature_selfdev_used) AS selfdev_sessions, SUM(feature_background_used) AS background_sessions, SUM(feature_subagent_used) AS subagent_sessions FROM events WHERE event IN ('session_end', 'session_crash') AND created_at > datetime('now', '-30 days')"

# Session success rate + abandonment rate (last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT AVG(CASE WHEN session_success > 0 THEN 1.0 ELSE 0.0 END) AS success_rate, AVG(CASE WHEN abandoned_before_response > 0 THEN 1.0 ELSE 0.0 END) AS abandoned_before_response_rate FROM events WHERE event IN ('session_end', 'session_crash') AND created_at > datetime('now', '-30 days')"

# Tool and response latency (last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT AVG(first_assistant_response_ms) AS avg_first_response_ms, AVG(first_tool_success_ms) AS avg_first_tool_success_ms, AVG(CASE WHEN executed_tool_calls > 0 THEN CAST(tool_latency_total_ms AS REAL) / executed_tool_calls END) AS avg_tool_latency_ms FROM events WHERE event IN ('session_end', 'session_crash') AND created_at > datetime('now', '-30 days')"

# --- Website + subscription analytics (requires 0016) ---

# Daily web visitors (distinct anonymous visitor_ids per UTC day, last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT date(e.created_at) AS day, COUNT(DISTINCT w.visitor_id) AS visitors, COUNT(*) AS pageviews FROM events e JOIN web_details w ON w.event_id = e.event_id WHERE e.event = 'web_pageview' AND e.created_at > datetime('now', '-30 days') GROUP BY day ORDER BY day"

# Pricing-page funnel: pageview -> CTA click by tier (last 30d).
# cta encodes the tier (plus_early_access / flagship_early_access / install).
wrangler d1 execute jcode-telemetry --command "WITH viewers AS (SELECT COUNT(DISTINCT w.visitor_id) AS n FROM events e JOIN web_details w ON w.event_id = e.event_id WHERE e.event = 'web_pageview' AND w.path = '/pricing' AND e.created_at > datetime('now', '-30 days')) SELECT w.cta, COUNT(DISTINCT w.visitor_id) AS clickers, (SELECT n FROM viewers) AS pricing_viewers, ROUND(1.0 * COUNT(DISTINCT w.visitor_id) / MAX(1, (SELECT n FROM viewers)), 4) AS click_through FROM events e JOIN web_details w ON w.event_id = e.event_id WHERE e.event = 'web_cta_click' AND w.path = '/pricing' AND e.created_at > datetime('now', '-30 days') GROUP BY w.cta ORDER BY clickers DESC"

# Traffic sources for pricing pageviews (last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT w.utm_source, w.utm_medium, w.utm_campaign, COUNT(DISTINCT w.visitor_id) AS visitors FROM events e JOIN web_details w ON w.event_id = e.event_id WHERE e.event = 'web_pageview' AND e.created_at > datetime('now', '-30 days') GROUP BY 1, 2, 3 ORDER BY visitors DESC"

# Subscription activations by tier (last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT tier, COUNT(DISTINCT account_id) AS accounts, COUNT(*) AS activations FROM events WHERE event = 'subscription_activated' AND created_at > datetime('now', '-30 days') GROUP BY tier ORDER BY accounts DESC"

# Budget exhaustion count (accounts hitting their token budget, by tier, last 30d)
wrangler d1 execute jcode-telemetry --command "SELECT tier, COUNT(*) AS exhaustion_events, COUNT(DISTINCT account_id) AS accounts FROM events WHERE event = 'subscription_budget_exhausted' AND created_at > datetime('now', '-30 days') GROUP BY tier ORDER BY exhaustion_events DESC"

# Subscription router errors by tier/model (last 7d)
wrangler d1 execute jcode-telemetry --command "SELECT tier, model_start AS model, COUNT(*) AS errors, COUNT(DISTINCT account_id) AS accounts FROM events WHERE event = 'subscription_router_error' AND created_at > datetime('now', '-7 days') GROUP BY 1, 2 ORDER BY errors DESC"

# account_linked join example: CLI usage (meaningful active days, last 30d)
# per subscribed account, via the telemetry_id <-> account_id anchor.
wrangler d1 execute jcode-telemetry --command "WITH links AS (SELECT DISTINCT telemetry_id, account_id FROM events WHERE event = 'account_linked') SELECT l.account_id, COUNT(DISTINCT d.activity_date) AS active_days_30d, SUM(d.turn_end_count) AS turns_30d FROM links l JOIN daily_active_users d ON d.telemetry_id = l.telemetry_id WHERE d.activity_date > date('now', '-30 days') AND d.meaningful_active > 0 GROUP BY l.account_id ORDER BY active_days_30d DESC LIMIT 50"
```

## What to watch for

- `session_start` far exceeding `session_end + session_crash` for multiple days
- `session_crash = 0` for long periods despite known crashes
- large `lifecycle_ids_without_install` counts
- a single telemetry ID dominating session totals (dev/test skew)
- zeroed transport totals after transport-aware releases (missing migration)
- `daily_active_users` row counts diverging from raw distinct-user checks
- headline DAU including `build_channel != 'release'` or raw event counts instead of distinct users
- headline DAU/installs including CI traffic (`is_ci = 1`); prefer the `*_noci` columns. A spike in `ci_ids_30d` / `ci_install_ids` from `health.sql` means CI runners are inflating user and install counts.

## Accuracy notes

- DAU/WAU/MAU should be distinct `telemetry_id` counts, never event counts. Heavy users and long-running agents can emit thousands of `turn_end` events in a day.
- Use `meaningful_release_active` for headline product usage. It excludes local/dev/git-checkout traffic and open/close sessions with no meaningful lifecycle activity.
- For the cleanest headline numbers, prefer the `*_noci` columns, which additionally exclude `is_ci = 1` traffic. Ephemeral CI runners mint a fresh `telemetry_id` per job, so unfiltered they look like brand-new users and installs, inflating active-user/install counts and depressing retention. The client also skips the `install` event under CI, so historical CI installs (before that ships) are the main residual source; the rollup's `last_is_ci` flag lets dashboards filter the rest. Raw events stay tagged (not dropped) so CI crash/error signal is still queryable.
- Meaningful activity is derived from `session_end`/`session_crash` **and** `turn_end` events. A `turn_end` only fires after a real user turn completes, so counting it keeps the metric accurate for users whose `session_end` is lost (process killed, machine shutdown, dropped final flush, or a session still open at UTC midnight).
- **Retention pruning**: D1 hard-caps databases at 500 MB. When the cap is hit, every insert fails with HTTP 500 and telemetry silently stops being recorded (this happened in June 2026; ~3 days of events were lost). The worker now runs a nightly cron (`scheduled` handler, see `RETENTION_DAYS` in `src/worker.js`) that prunes high-volume raw rows: `turn_end`/`session_start`/`onboarding_step` after 30 days, `upgrade` after 60, `auth_success` after 180, `session_end`/`session_crash` after 365, `web_pageview`/`subscription_router_error` after 90, `web_cta_click`/`subscription_budget_exhausted` after 365, `subscription_login` after 180. `install`, `feedback`, `subscription_activated`, and `account_linked` rows are never pruned.  Because of this, **historical user/DAU queries must read `daily_active_users`, not raw `events`** - the rollup is backfilled across full history (migration 0014) and maintained at insert time.
- **D1 100-column cap**: production `events` has 98 columns after migration 0016 and D1 refuses `ALTER TABLE ADD COLUMN` past 100 (`too many columns`). Migration 0005's per-turn/session-cadence columns never applied to production `events`; migration 0013 moved those fields into `turn_details`/`session_details`, and migration 0016 put the web beacon fields in `web_details` for the same reason. Do not add new columns to `events`; add them to the detail tables.
- Raw events remain the source of truth within their retention windows. The `daily_active_users` table is an ingest-time rollup for cheap dashboard queries and is the durable record beyond those windows.
- The worker uses `INSERT OR IGNORE` keyed by `event_id`; rollups and detail rows are updated only when the canonical raw event insert succeeds, so client retries do not inflate counts.
- Telemetry still undercounts users who opt out (`JCODE_NO_TELEMETRY`, `DO_NOT_TRACK`, `~/.jcode/no_telemetry`) or whose network blocks telemetry, and may overcount one person using multiple machines.
