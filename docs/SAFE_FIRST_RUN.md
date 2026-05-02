# Safe First Run Guide

> **Purpose:** Evaluate jcode safely before granting it access to your primary machine, credentials, or sensitive repositories.

jcode is a powerful coding-agent harness with high-trust capabilities. This guide helps new users evaluate jcode safely.

## Quick Start: Safe Evaluation

Run jcode with conservative defaults using an isolated home directory:

```bash
# Create isolated evaluation environment
export JCODE_HOME="$HOME/.jcode-sandbox"
mkdir -p "$JCODE_HOME"
chmod 700 "$JCODE_HOME"

# Disable telemetry for privacy-sensitive testing
export JCODE_NO_TELEMETRY=1
export DO_NOT_TRACK=1

# Launch with no ambient mode or self-dev by default
jcode
```

## High-Impact Capabilities

jcode combines many powerful capabilities. Understand each before enabling:

| Capability | Risk Level | Description |
|------------|------------|-------------|
| Shell execution | **High** | Runs commands in your terminal |
| File writes | **High** | Creates, modifies, or deletes files |
| Git operations | **Medium-High** | Commits, pushes, creates branches |
| Browser automation | **High** | Controls browser with your sessions |
| Provider credentials | **High** | Access to your API keys and OAuth tokens |
| MCP integrations | **Medium** | Runs third-party tools you configure |
| Ambient/autonomous mode | **High** | Operates without direct supervision |
| Persistent memory | **Low** | Stores conversation context locally |
| Self-dev mode | **High** | Modifies jcode's own source code |

## Evaluation Checklist

Before your first evaluation, consider:

### 1. Use an Isolated Environment

```bash
# Use a dedicated directory for jcode data
export JCODE_HOME="$HOME/.jcode-eval"

# Or use a worktree/container/VM
git worktree add /tmp/jcode-eval <repository>
```

### 2. Avoid Primary Credentials

- **Don't** connect jcode to your primary API accounts initially
- **Do** create a separate API key for testing
- **Do** use a test project, not your main codebase

```bash
# Example: Test with a separate provider profile
jcode provider add test-provider \
  --base-url http://localhost:8000/v1 \
  --model test-model \
  --no-api-key
```

### 3. Disable Sensitive Integrations Initially

```bash
# Skip these during first evaluation:
# - Gmail/Google OAuth
# - Personal browser automation
# - Sensitive MCP servers
# - Ambient autonomous mode
```

### 4. Use Disposal Repositories

```bash
# Create a test repo for evaluation
mkdir -p /tmp/jcode-test-repo
cd /tmp/jcode-test-repo
git init

# Never run jcode on sensitive repos during evaluation
```

### 5. Opt Out of Telemetry

```bash
export JCODE_NO_TELEMETRY=1
export DO_NOT_TRACK=1
```

See [TELEMETRY.md](../TELEMETRY.md) for details on what data is collected.

## Environment Variables for Safe Evaluation

| Variable | Purpose |
|----------|---------|
| `JCODE_HOME` | Override default config directory |
| `JCODE_NO_TELEMETRY=1` | Disable telemetry |
| `DO_NOT_TRACK=1` | Additional telemetry opt-out |
| `JCODE_NO_AMBIENT=1` | Disable ambient mode |
| `JCODE_NO_SELFDEV=1` | Disable self-dev mode |

## Tool Confirmation Prompts

When running in interactive mode, jcode will prompt for confirmation on high-impact actions. You can also configure explicit approval requirements:

```toml
# In ~/.jcode/config.toml
[safety]
# Require explicit approval for all shell commands
require_approval_for_shell = true
require_approval_for_write = true
require_approval_for_git_push = true
```

## Credential Auto-Import

jcode may offer to import credentials from other tools (Claude Code, OpenCode, etc.). During first evaluation, review these carefully:

```bash
# Check what credentials jcode has access to
jcode config list --secrets

# Clear all imported credentials
rm -rf ~/.jcode/auth.json
```

## Monitoring jcode Activity

### View Recent Commands

```bash
# Check session transcripts
ls ~/.jcode/sessions/

# View specific session
jcode session log <session-name>
```

### Audit Enabled Tools

```bash
# List configured MCP servers
jcode mcp list

# Disable a specific MCP server
jcode mcp remove <server-name>
```

## Cleaning Up After Evaluation

If you decide jcode isn't right for your use case, or want a clean slate:

```bash
# Stop all jcode processes
pkill -f jcode

# Remove all jcode data
rm -rf ~/.jcode

# Remove from PATH (if installed via script)
rm /usr/local/bin/jcode
```

## Related Documentation

- [Safety System](SAFETY_SYSTEM.md) - Human-in-the-loop safety for ambient operations
- [Telemetry](TELEMETRY.md) - What data jcode collects
- [Ambient Mode](AMBIENT_MODE.md) - Unsupervised agent operation
- [OAuth Setup](OAUTH.md) - Provider authentication

---

*This guide helps users evaluate jcode safely. As you become familiar with jcode's capabilities, you can enable additional features based on your trust level.*
