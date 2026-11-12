#!/usr/bin/env bash
# agent-sync-recovery.sh — run from the LAPTOP after the desktop sshd is
# restored. This seeds the desk with the laptop's jcode/codex/forge config
# in three waves:
#
#   1. jcode-* (entirely laptop-only — desk has no .jcode)
#   2. skills + memories + AGENTS.md (content; safe to merge)
#   3. mcp configs (last; safe to clobber with the canonical laptop copy)
#
# Run each wave in order, checking output between them.
#
# Usage:  bash agent-sync-recovery.sh

set -euo pipefail

AS="${AS:-$HOME/.local/bin/agent-sync}"

echo "==> 1) jcode (laptop is the source of truth)"
"$AS" --push-only jcode
echo
echo "==> 2) skills / memories / AGENTS.md"
"$AS" jcode-memories
"$AS" codex-skills
"$AS" codex-memories
"$AS" codex-agents-md
"$AS" forge-commands
"$AS" forge-sessions
echo
echo "==> 3) MCP configs"
"$AS" --push-only jcode-mcp
"$AS" --push-only codex-mcp
"$AS" --push-only codex-mcp-json
"$AS" --push-only forge-mcp
echo
echo "==> 4) config files (manual — review before running)"
echo "    \$AS --include-manual --push-only jcode-config   # only if desk .jcode/config.toml should be laptop's"
echo "    \$AS --include-manual --push-only codex-config   # ditto for codex"
echo "    \$AS --include-manual --push-only forge-config   # ditto for forge"
echo
echo "==> done. verify with: agent-sync --status"
