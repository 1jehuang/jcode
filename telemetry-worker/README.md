# jcode Telemetry Worker

Cloudflare Worker that receives anonymous telemetry events from jcode.

The headline number is **Total users**: distinct, non-CI `telemetry_id`s that
ever installed jcode OR did meaningful work in it. Run it with:

```bash
wrangler d1 execute jcode-telemetry --remote --file=users.sql
```

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

# Run the health dashboard query
npm run health
```

## Querying Data

```bash
# Total installs (raw, and excluding CI runners which mint a fresh id per job)
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) AS raw_installs, COUNT(DISTINCT CASE WHEN is_ci = 0 THEN telemetry_id END) AS installs_noci FROM events WHERE event = 'install'"

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

# Error rates
wrangler d1 execute jcode-telemetry --command "SELECT SUM(error_provider_timeout) as timeouts, SUM(error_rate_limited) as rate_limits, SUM(error_auth_failed) as auth_failures FROM events WHERE event = 'session_end'"

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
- Raw events remain the source of truth. The `daily_active_users` table is an ingest-time rollup for cheap dashboard queries and is backfillable from `events`.
- The worker uses `INSERT OR IGNORE` keyed by `event_id`; rollups and detail rows are updated only when the canonical raw event insert succeeds, so client retries do not inflate counts.
- Telemetry still undercounts users who opt out (`JCODE_NO_TELEMETRY`, `DO_NOT_TRACK`, `~/.jcode/no_telemetry`) or whose network blocks telemetry, and may overcount one person using multiple machines.
