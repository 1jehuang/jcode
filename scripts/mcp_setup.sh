#!/bin/bash
# CarpAI MCP Server Setup Script
# Installs dependencies and configures MCP servers

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
MCP_DIR="$PROJECT_DIR/mcp-servers"

echo "=== CarpAI MCP Setup ==="
echo ""

# Check Python
if ! command -v python3 &>/dev/null; then
    echo "ERROR: python3 not found"
    exit 1
fi

# Check for .env file
ENV_FILE="$PROJECT_DIR/.env.mcp"
if [ -f "$ENV_FILE" ]; then
    echo "Loading environment from $ENV_FILE"
    set -a
    source "$ENV_FILE"
    set +a
fi

# Install base dependencies
echo "Installing base MCP dependencies..."
pip install -r "$MCP_DIR/requirements.txt"

# Install server-specific dependencies based on available env vars
install_if_configured() {
    local name="$1"
    local req_file="$MCP_DIR/requirements-$name.txt"
    local vars=("${@:2}")

    for var in "${vars[@]}"; do
        if [ -z "${!var}" ]; then
            echo "[SKIP] $name: $var not set"
            return
        fi
    done

    if [ -f "$req_file" ]; then
        echo "[INSTALL] $name dependencies..."
        pip install -r "$req_file"
    fi
}

install_if_configured "github" "GITHUB_TOKEN"
install_if_configured "jira" "JIRA_URL" "JIRA_API_TOKEN"
install_if_configured "slack" "SLACK_BOT_TOKEN"
install_if_configured "postgres" "DATABASE_URL"
install_if_configured "redis" "REDIS_URL"
install_if_configured "kubernetes" "KUBECONFIG"
install_if_configured "aws" "AWS_ACCESS_KEY_ID"
install_if_configured "sentry" "SENTRY_TOKEN"
install_if_configured "datadog" "DATADOG_API_KEY" "DATADOG_APP_KEY"

echo ""
echo "=== Setup complete ==="
echo "Run 'python mcp-servers/start_all.py' to start configured servers"
