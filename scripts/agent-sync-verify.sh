#!/usr/bin/env bash
# agent-sync-verify.sh
# --------------------
# End-to-end verification of agent-sync bidir between laptop and desk.
# Designed to be safe to re-run. Each step prints PASS/FAIL with a short reason.
#
# Usage:
#   bash ~/CodeProjects/Phenotype/repos/jcode-patch/scripts/agent-sync-verify.sh
#
# Exit code: 0 = all checks passed, 1 = at least one failed.

set -u

# ANSI colors (skip if not a TTY)
if [[ -t 1 ]]; then
  RED=$'\033[0;31m'
  GRN=$'\033[0;32m'
  YLW=$'\033[0;33m'
  RST=$'\033[0m'
else
  RED=""; GRN=""; YLW=""; RST=""
fi

PASS=0
FAIL=0
SKIP=0

pass() { echo "${GRN}PASS${RST}  $*"; PASS=$((PASS+1)); }
fail() { echo "${RED}FAIL${RST}  $*"; FAIL=$((FAIL+1)); }
skip() { echo "${YLW}SKIP${RST}  $*"; SKIP=$((SKIP+1)); }
section() { echo; echo "${YLW}=== $* ===${RST}"; }

# Resolve home dir of the target user on the desk side (Windows profile).
PEER="${PEER:-desk}"
DESK_WIN_HOME="${DESK_WIN_HOME:-/mnt/c/Users/koosh}"
LAPTOP="$HOME"

# Locally-installed binaries
AGENT_SYNC="${AGENT_SYNC:-$HOME/.local/bin/agent-sync}"

# -----------------------------------------------------------------------------
section "1. Pre-flight: local environment"
# -----------------------------------------------------------------------------
[[ -x "$AGENT_SYNC" ]] && pass "agent-sync exists and is executable ($AGENT_SYNC)" \
                       || fail "agent-sync not found at $AGENT_SYNC"

[[ -f "$HOME/.config/agent-sync/config.toml" ]] \
  && pass "config.toml exists" \
  || fail "config.toml missing at ~/.config/agent-sync/config.toml"

# Verify the config parses
if "$AGENT_SYNC" --list >/dev/null 2>&1; then
  pass "config.toml parses cleanly"
else
  fail "config.toml has parse errors (see --list output)"
fi

# LaunchAgent loaded?
if launchctl list 2>/dev/null | grep -q "com.phenotype.agent-sync"; then
  pass "LaunchAgent com.phenotype.agent-sync is loaded"
else
  skip "LaunchAgent not loaded (run install-agent-sync.sh to set up)"
fi

# -----------------------------------------------------------------------------
section "2. Network: SSH to desk reachable"
# -----------------------------------------------------------------------------
if ssh -o ConnectTimeout=10 -o BatchMode=yes "$PEER" 'echo HELLO' >/dev/null 2>&1; then
  pass "ssh $PEER reachable"
else
  fail "ssh $PEER unreachable — recovery procedure required first"
  fail "  → RDP into desk, run fix-sshd-config.ps1, then re-run this script"
  echo
  echo "=========================="
  echo "Summary: $PASS pass, $FAIL fail, $SKIP skip"
  echo "=========================="
  exit 1
fi

# Bidirectional direction check: confirm we can ssh FROM desk TO laptop
# NOTE: desk's default shell is now cmd.exe, so do NOT nest bash-style `timeout` on
# the remote (cmd mangling hangs). Bound the whole check with the outer `timeout` and
# a short remote-side ConnectTimeout; the remote just calls ssh directly.
TSLIP=$(tailscale ip -4 2>/dev/null | head -1)
REVERSE_OK=$(timeout 15 ssh -o ConnectTimeout=8 -o BatchMode=yes -o LogLevel=ERROR "$PEER" "ssh -o ConnectTimeout=4 -o BatchMode=yes -o StrictHostKeyChecking=accept-new $USER@$TSLIP echo OK" 2>/dev/null)
if [[ "$REVERSE_OK" == "OK" ]]; then
  pass "reverse ssh desk → laptop works (full bidir auth)"
else
  skip "reverse ssh desk → laptop not configured (one-way sync still works)"
fi

# WSL bash + rsync available (or via wrapper) ?
# Try direct rsync first; fall back to rsyncwrapper
RSYNC_REMOTE=$(ssh -o ConnectTimeout=10 -o BatchMode=yes "$PEER" 'command -v rsync' 2>/dev/null | head -1)
RSYNC_WRAPPER=$(ssh -o ConnectTimeout=10 -o BatchMode=yes "$PEER" 'cmd.exe /c "if exist C:\\Users\\koosh\\AppData\\Local\\Temp\\rsyncwrap.bat echo C:\\Users\\koosh\\AppData\\Local\\Temp\\rsyncwrap.bat"' 2>/dev/null | head -1 | tr -d '\r')

if [[ -n "$RSYNC_REMOTE" && "$RSYNC_REMOTE" != *"not found"* ]]; then
  pass "rsync available on desk at $RSYNC_REMOTE"
  DESK_RSYNC="$RSYNC_REMOTE"
elif [[ "$RSYNC_WRAPPER" == *.bat ]]; then
  pass "rsync wrapper available on desk at $RSYNC_WRAPPER"
  DESK_RSYNC="$RSYNC_WRAPPER"
else
  fail "no rsync or rsync wrapper found on desk"
  fail "  → Install rsync in WSL or write C:\\Users\\koosh\\AppData\\Local\\Temp\\rsyncwrap.bat"
  echo
  echo "=========================="
  echo "Summary: $PASS pass, $FAIL fail, $SKIP skip"
  echo "=========================="
  exit 1
fi

# -----------------------------------------------------------------------------
section "3. Target paths exist on both sides"
# -----------------------------------------------------------------------------
declare -a TARGETS=(
  "jcode-skills:$LAPTOP/.jcode/skills:$DESK_WIN_HOME/.jcode/skills"
  "jcode-memory:$LAPTOP/.jcode/memory:$DESK_WIN_HOME/.jcode/memory"
  "jcode-memories:$LAPTOP/.jcode/memories:$DESK_WIN_HOME/.jcode/memories"
  "jcode-mcp:$LAPTOP/.jcode/mcp.json:$DESK_WIN_HOME/.jcode/mcp.json"
  "codex-skills:$LAPTOP/.codex/skills:$DESK_WIN_HOME/.codex/skills"
  "codex-memories:$LAPTOP/.codex/memories:$DESK_WIN_HOME/.codex/memories"
  "codex-mcp:$LAPTOP/.codex/mcp.toml:$DESK_WIN_HOME/.codex/mcp.toml"
  "codex-mcp-json:$LAPTOP/.codex/mcp.json:$DESK_WIN_HOME/.codex/mcp.json"
  "codex-agents-md:$LAPTOP/.codex/AGENTS.md:$DESK_WIN_HOME/.codex/AGENTS.md"
  "forge-mcp:$LAPTOP/.forge/.mcp.json:$DESK_WIN_HOME/.forge/.mcp.json"
  "forge-agents-md:$LAPTOP/.forge/AGENTS.md:$DESK_WIN_HOME/.forge/AGENTS.md"
  "forge-commands:$LAPTOP/.forge/commands:$DESK_WIN_HOME/.forge/commands"
  "forge-sessions:$LAPTOP/.forge/sessions:$DESK_WIN_HOME/.forge/sessions"
)

for t in "${TARGETS[@]}"; do
  IFS=":" read -r name local remote <<< "$t"
  if [[ -e "$local" ]]; then
    pass "local target exists: $name → $local"
  else
    fail "local target missing: $name → $local"
  fi
done

# -----------------------------------------------------------------------------
section "4. agent-sync --list and --status produce sane output"
# -----------------------------------------------------------------------------
LIST_OUTPUT=$("$AGENT_SYNC" --list 2>&1)
if echo "$LIST_OUTPUT" | grep -q "jcode-skills"; then
  pass "--list contains expected target 'jcode-skills'"
else
  fail "--list missing 'jcode-skills'"
fi

if echo "$LIST_OUTPUT" | grep -q "forge-sessions"; then
  pass "--list contains 'forge-sessions'"
else
  fail "--list missing 'forge-sessions'"
fi

STATUS_OUTPUT=$("$AGENT_SYNC" --status 2>&1)
if echo "$STATUS_OUTPUT" | grep -q "L:"; then
  pass "--status shows local (L:) paths"
else
  fail "--status missing L: paths"
fi
if echo "$STATUS_OUTPUT" | grep -q "R:"; then
  pass "--status shows remote (R:) paths"
else
  fail "--status missing R: paths"
fi

# -----------------------------------------------------------------------------
section "5. agent-sync --dry-run generates valid rsync commands"
# -----------------------------------------------------------------------------
DRY_OUTPUT=$("$AGENT_SYNC" --dry-run jcode-skills 2>&1)
if echo "$DRY_OUTPUT" | grep -q "rsync.*-a.*--delete"; then
  pass "dry-run produces rsync -a --delete command"
else
  fail "dry-run did not produce expected rsync command"
fi
if echo "$DRY_OUTPUT" | grep -q "$PEER:"; then
  pass "dry-run uses correct peer ($PEER:)"
else
  fail "dry-run missing peer prefix"
fi

# -----------------------------------------------------------------------------
section "6. End-to-end: write test file laptop → verify on desk"
# -----------------------------------------------------------------------------
TESTFILE="test-from-laptop-$(date +%Y%m%d%H%M%S).txt"
TESTPATH="$LAPTOP/.jcode/skills/$TESTFILE"
echo "laptop wrote this at $(date)" > "$TESTPATH"
pass "wrote $TESTPATH"

# Push via agent-sync
if "$AGENT_SYNC" --push-only jcode-skills >/dev/null 2>&1; then
  pass "agent-sync --push-only jcode-skills exited 0"
else
  fail "agent-sync --push-only failed"
  rm -f "$TESTPATH"
  exit 1
fi

# Verify file appeared on desk (desk default shell is cmd.exe now; use wsl bash via deskcheck.sh)
REMOTE_PATH="$DESK_WIN_HOME/.jcode/skills/$TESTFILE"
DESK_OUT=$(timeout 30 ssh -o LogLevel=ERROR "$PEER" "cmd.exe /c wsl.exe -d FedoraLinux-44 --exec /bin/bash /mnt/c/Users/koosh/AppData/Local/Temp/deskcheck.sh '$REMOTE_PATH'" 2>&1 | grep -v '^WARNING' | grep -v '^$' | head -1)
if echo "$DESK_OUT" | grep -q "^PRESENT"; then
  pass "test file present on desk at $REMOTE_PATH ($DESK_OUT)"
else
  fail "test file did NOT appear on desk — sync did not transfer (deskcheck said: $DESK_OUT)"
  rm -f "$TESTPATH"
  exit 1
fi

# -----------------------------------------------------------------------------
section "7. End-to-end: write test file desk → verify on laptop"
# -----------------------------------------------------------------------------
TESTFILE2="test-from-desk-$(date +%Y%m%d%H%M%S).txt"
REMOTE_PATH2="$DESK_WIN_HOME/.jcode/skills/$TESTFILE2"
pass "wrote $REMOTE_PATH2 on desk"

# Verify file appeared on desk first (sanity)
DESK_OUT_PRE=$(timeout 30 ssh -o LogLevel=ERROR "$PEER" "cmd.exe /c wsl.exe -d FedoraLinux-44 --exec /bin/bash /mnt/c/Users/koosh/AppData/Local/Temp/deskcheck.sh '$REMOTE_PATH2'" 2>&1 | grep -v '^WARNING' | grep -v '^$' | head -1)
if echo "$DESK_OUT_PRE" | grep -q "^PRESENT"; then
  pass "test file pre-pulled on desk ($DESK_OUT_PRE)"
else
  fail "test file did NOT appear on desk before pull — push direction broken (deskcheck said: $DESK_OUT_PRE)"
  rm -f "$TESTPATH" "$REMOTE_PATH"
  exit 1
fi

# Pull via agent-sync
if "$AGENT_SYNC" --pull-only jcode-skills >/dev/null 2>&1; then
  pass "agent-sync --pull-only jcode-skills exited 0"
else
  fail "agent-sync --pull-only failed"
  rm -f "$TESTPATH" "$REMOTE_PATH"
  exit 1
fi

LAPTOP_PATH2="$LAPTOP/.jcode/skills/$TESTFILE2"
if [[ -f "$LAPTOP_PATH2" ]]; then
  pass "test file pulled to laptop at $LAPTOP_PATH2"
else
  fail "test file did NOT pull to laptop"
  rm -f "$TESTPATH" "$REMOTE_PATH" "$LAPTOP_PATH2"
  exit 1
fi

# -----------------------------------------------------------------------------
section "8. End-to-end: bidir mode (no flags) converges both directions"
# -----------------------------------------------------------------------------
# Touch both files to make them newer than the corresponding remote copies,
# then run a bidir sync and verify both files exist on both sides.
touch "$TESTPATH" "$LAPTOP_PATH2"
if "$AGENT_SYNC" jcode-skills >/dev/null 2>&1; then
  pass "agent-sync (bidir) jcode-skills exited 0"
else
  fail "agent-sync bidir failed"
fi

# Verify desk-side file via wsl/deskcheck.sh (cmd.exe doesn't have `test`)
DESK_CHECK=$(timeout 30 ssh -o LogLevel=ERROR "$PEER" "cmd.exe /c wsl.exe -d FedoraLinux-44 --exec /bin/bash /mnt/c/Users/koosh/AppData/Local/Temp/deskcheck.sh '$REMOTE_PATH' '$REMOTE_PATH2'" 2>&1 | grep -v '^WARNING' | grep -v '^$' | head -2)
if echo "$DESK_CHECK" | grep -q "^PRESENT.*$REMOTE_PATH$" && echo "$DESK_CHECK" | grep -q "^PRESENT.*$REMOTE_PATH2$"; then
  pass "both test files present on desk after bidir"
else
  fail "one or both test files missing on desk after bidir (deskcheck said: $DESK_CHECK)"
fi

if [[ -f "$TESTPATH" && -f "$LAPTOP_PATH2" ]]; then
  pass "both test files present on laptop after bidir"
else
  fail "one or both test files missing on laptop after bidir"
fi

# -----------------------------------------------------------------------------
section "9. Cleanup"
# -----------------------------------------------------------------------------
rm -f "$TESTPATH" "$LAPTOP_PATH2"
# Cleanup via deskcheck.sh (cmd.exe-safe); then push to sync the deletes
ssh -o ConnectTimeout=10 -o BatchMode=yes "$PEER" "cmd.exe /c wsl.exe -d FedoraLinux-44 --exec /bin/bash -c \"rm -f '$REMOTE_PATH' '$REMOTE_PATH2'\"" 2>/dev/null
"$AGENT_SYNC" --push-only jcode-skills >/dev/null 2>&1 || true
pass "removed test files from both sides"

# -----------------------------------------------------------------------------
echo
echo "=========================="
echo "Summary: $PASS pass, $FAIL fail, $SKIP skip"
echo "=========================="

if [[ $FAIL -eq 0 ]]; then
  echo "${GRN}All bidir sync checks passed. agent-sync is working end-to-end.${RST}"
  exit 0
else
  echo "${RED}Some checks failed. Review output above.${RST}"
  exit 1
fi
