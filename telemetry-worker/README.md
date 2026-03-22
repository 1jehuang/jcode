# jcode Telemetry Worker

Cloudflare Worker that receives anonymous telemetry events from jcode.

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
apply the expansion migration remotely:

```bash
wrangler d1 execute jcode-telemetry --remote --file=migrations/0001_expand_events.sql
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

## Querying Data

```bash
# Total installs
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) FROM events WHERE event = 'install'"

# Raw active users this week
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) FROM events WHERE event = 'session_end' AND created_at > datetime('now', '-7 days')"

# Meaningful active users this week (filters out empty open/close sessions)
wrangler d1 execute jcode-telemetry --command "SELECT COUNT(DISTINCT telemetry_id) FROM events WHERE event = 'session_end' AND created_at > datetime('now', '-7 days') AND (turns > 0 OR duration_mins > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0 OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0 OR provider_switches > 0 OR model_switches > 0)"

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
```
