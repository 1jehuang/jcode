# Status of the docs in this folder

The goal: give jcode the same experience as Claude Code's `/mcp` and `/skills`
(and the same skill/MCP behaviour for the model). The numbered task docs and
phase docs were written for an earlier "modularize the skill/MCP code" plan.
Each is listed below against that goal.

| Doc | Status | Notes |
|---|---|---|
| `claude-code-parity.md` | Current | How Claude Code works, what jcode had, what was built. |
| `claude-code-ui-reference.md` | Current | Captured Claude Code 2.1.302 `/mcp` and `/skills` screens. |
| 01, 02, 04, 06, 08 (research) | Done | Answered in `claude-code-parity.md` Part 2. |
| 03, 05, 07 (input parsing research) | Moot | Parsing internals; nothing the goal needs beyond what is covered. |
| 09–12 (architecture, boundaries, contracts, test strategy) | Moot | Design artifacts for the refactor. |
| 13–15 (extract modules) | Moot | Refactor only. 13 was attempted, was broken, and is in `git stash@{0}`. |
| 16 (doc headers) | Moot | New files have module headers. |
| 17–18 (unit/integration tests) | Done for the goal | Tests cover the dialogs, overrides, approvals, frontmatter, plugin naming, announcements. |
| 19 (end-to-end) | Done manually | Both dialogs driven live in tmux against a daemon on a private socket, side by side with Claude Code. |
| 20, 24 (performance) | Moot | No hot path changed in a way that needs a benchmark; the MCP status check is throttled to once a minute. |
| 21 (user docs) | Done | `docs/SKILLS_AND_MCP.md` (bundled into `jcode_docs`). |
| 22 (developer docs) | Moot | `claude-code-parity.md` is the design record. |
| 23 (security audit) | Done for the goal | Project `.mcp.json` servers now need approval; approvals live outside the repo. |
| 25 (deployment validation) | Partial | Builds and tests pass; nothing committed or installed yet. |
| `phase-1` … `phase-5` | As their tasks above | |
| `IMPLEMENTATION_SUMMARY.md` | Replaced | Now summarizes the parity work instead of the abandoned refactor. |
