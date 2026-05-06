# MCP Plan

MCP setup is intentionally review-first. This init command does not download or install MCP servers automatically.

## Recommended review steps

1. Identify required systems: filesystem, GitHub, browser, database, issue tracker, docs, deployment.
2. Prefer local/offline MCP servers when possible.
3. Document credential requirements and never commit secrets.
4. Add reviewed server definitions to `.jcode/mcp.json`.
5. Validate with `jcode` after reviewing permissions.

## Candidate server categories

- Filesystem/code search: usually already covered by native jcode tools.
- Browser/Playwright: useful for UI QA.
- GitHub/GitLab: useful for issues/PRs, requires tokens.
- Database: useful for diagnostics, requires strict read/write boundaries.
- Docs/search: useful, may require network.
