# MCP Plan

MCP setup is intentionally review-first. This init command does not download or install MCP servers automatically.

## Current project state

- `.jcode/mcp.json` currently has no active servers: `{ "mcpServers": {} }`.
- Native jcode tools already cover filesystem reads/search/edits for normal repository work.
- Do not store tokens, API keys, `.env` values, private keys, deployment credentials, or database credentials in memory, docs, side-panel pages, or generated reports.

## Candidate server categories

Review in this order and enable only with explicit permission and documented scopes:

1. Browser/Playwright: useful for UI QA, docs screenshots, and web-flow validation.
2. GitHub/GitLab: useful for issues, PRs, and release automation. Requires token scoping and secret handling.
3. Docs/search: useful for network-backed research. Requires network boundary review.
4. Database/telemetry: useful for diagnostics only with strict read/write boundaries and credential isolation.
5. Filesystem/code search: usually unnecessary because native jcode tools already cover this repository.

## Tooling/security evidence

- `telemetry-worker/package.json` uses `npx wrangler` for dev, deploy, health, and remote D1 migrations.
- No active MCP servers are configured in `.jcode/mcp.json`.
- Discovery reported no Node lockfile for `telemetry-worker`; review package-manager reproducibility before treating telemetry deployment as release-gated.
- Workflows use deployment secrets and third-party actions; release workflow has write permissions. Keep MCP and automation credentials separate from repository files.
- No `pull_request_target` workflow trigger was reported.

## Recommended review steps

1. Identify required systems: browser, GitHub, docs/search, database/telemetry, deployment.
2. Prefer local/offline MCP servers when possible.
3. Document credential requirements and never commit secrets.
4. Add reviewed server definitions to `.jcode/mcp.json` only after permission review.
5. Validate with `jcode` after reviewing permissions.
