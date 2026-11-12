#!/usr/bin/env bash
# install-agent-sync.sh — install agent-sync + LaunchAgent on laptop,
# and stage the scheduled-task XML for the desktop (Windows side).
#
# After SSH to desk is restored, run the "post-restore" steps below.

set -eu

LAPTOP_HOME="${HOME:-/Users/<REDACTED>}"
LAPTOP_BIN="$LAPTOP_HOME/.local/bin/agent-sync"
LAPTOP_CFG="$LAPTOP_HOME/.config/agent-sync/config.toml"
LAPTOP_PLIST="$LAPTOP_HOME/Library/LaunchAgents/com.phenotype.agent-sync.plist"
LOG_DIR="$LAPTOP_HOME/Library/logs/agent-sync"
REPO="${REPO:-$LAPTOP_HOME/CodeProjects/Phenotype/repos/jcode-patch}"

echo "== install-agent-sync =="
echo "[1/4] ensure dirs"
mkdir -p "$(dirname "$LAPTOP_BIN")" \
         "$(dirname "$LAPTOP_CFG")" \
         "$(dirname "$LAPTOP_PLIST")" \
         "$LOG_DIR"

echo "[2/4] install agent-sync script + config"
install -m 0755 "$REPO/scripts/agent-sync" "$LAPTOP_BIN"
install -m 0644 "$REPO/scripts/agent-sync-config.toml" "$LAPTOP_CFG"

echo "[3/4] install LaunchAgent plist"
install -m 0644 "$REPO/scripts/com.phenotype.agent-sync.plist" "$LAPTOP_PLIST"
# Unload if already loaded, then load with -w so it persists across reboots
launchctl unload "$LAPTOP_PLIST" 2>/dev/null || true
launchctl load -w "$LAPTOP_PLIST"

echo "[4/4] summary"
echo "  agent-sync:  $(command -v agent-sync || echo "$LAPTOP_BIN")"
echo "  config:      $LAPTOP_CFG"
echo "  LaunchAgent: $LAPTOP_PLIST"
echo "  logs:        $LOG_DIR"

cat <<'NEXT'

== post-restore (run AFTER SSH to desk is back) ==

# 1. From laptop: push jcode first (only on laptop), then enable bidir
agent-sync --push-only jcode-skills jcode-memory jcode-mcp jcode-agents

# 2. From laptop: push codex (configs marked manual are NOT touched)
agent-sync --push-only codex-skills codex-memories codex-mcp codex-agents

# 3. From laptop: push forge
agent-sync --push-only forge-mcp forge-agents forge-commands forge-sessions

# 4. Switch to bidir mode (mtime newer wins; safe for ongoing edits)
agent-sync

# 5. Stage desktop side: copy agent-sync + config to C:\Users\koosh\.local\bin
#    and C:\Users\koosh\.config\agent-sync respectively, then register
#    the scheduled task from scheduled-task.xml (also in scripts/).

NEXT
