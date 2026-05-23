#!/bin/bash
# CarpAI Understand-Anything Plugin Installer
# 移植自: Understand-Anything install.sh (14平台兼容)
# 用法: bash scripts/install_knowledge_agent.sh [platform]
# 平台: claude-code, cursor, codex, opencode, gemini-cli, copilot, hermes, cline, kimi

set -euo pipefail

CARPAI_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_NAME="carpai-knowledge-agent"
PLATFORM="${1:-auto}"

# 颜色
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}━━━ CarpAI Knowledge Agent Installer ━━━${NC}"
echo -e "${BLUE}移植自: Understand-Anything (14 platforms)${NC}"
echo ""

# 检测平台
detect_platform() {
    if [ -n "${CLAUDE_CODE:-}" ] || command -v claude &>/dev/null; then
        echo "claude-code"
    elif [ -n "${CURSOR:-}" ] || [ -d ".cursor" ]; then
        echo "cursor"
    elif [ -n "${CODELY:-}" ] || [ -d ".codely" ]; then
        echo "codex"
    elif [ -n "${OPENCODE:-}" ] || [ -d ".opencode" ]; then
        echo "opencode"
    elif [ -n "${GEMINI_CLI:-}" ] || command -v gemini &>/dev/null; then
        echo "gemini-cli"
    elif [ -d ".vscode" ] || [ -n "${VSCODE_CWD:-}" ]; then
        echo "vscode"
    elif [ -n "${HERMES_AGENT:-}" ] || [ -d ".hermes" ]; then
        echo "hermes"
    elif [ -n "${CLINE:-}" ] || [ -d ".cline" ]; then
        echo "cline"
    elif [ -n "${KIMI:-}" ] || [ -d ".kimi" ]; then
        echo "kimi"
    else
        echo "unknown"
    fi
}

if [ "$PLATFORM" = "auto" ]; then
    PLATFORM=$(detect_platform)
    echo -e "${YELLOW}Detected platform: ${PLATFORM}${NC}"
fi

# 安装函数
install_for_claude_code() {
    echo -e "${GREEN}[Claude Code] Installing knowledge agent...${NC}"
    mkdir -p "$CARPAI_ROOT/.claude/plugins"
    cat > "$CARPAI_ROOT/.claude/plugins/$PLUGIN_NAME.json" << JSONEOF
{
  "name": "$PLUGIN_NAME",
  "description": "Codebase knowledge graph and guided tour",
  "version": "1.0.0",
  "commands": {
    "understand": "jcode understand",
    "understand-domain": "jcode understand --domain",
    "understand-onboard": "jcode understand --tour",
    "understand-diff": "jcode understand --diff",
    "understand-knowledge": "jcode understand --knowledge"
  }
}
JSONEOF
    echo -e "${GREEN}  ✅ Plugin registered for Claude Code${NC}"
}

install_for_cursor() {
    echo -e "${GREEN}[Cursor] Installing knowledge agent...${NC}"
    mkdir -p "$CARPAI_ROOT/.cursor"
    cat > "$CARPAI_ROOT/.cursor/plugins/carpai-knowledge-agent.json" << JSONEOF
{
  "name": "$PLUGIN_NAME",
  "description": "Codebase knowledge graph and guided tour",
  "version": "1.0.0",
  "entry": "jcode understand"
}
JSONEOF
    echo -e "${GREEN}  ✅ Plugin registered for Cursor${NC}"
}

install_for_vscode() {
    echo -e "${GREEN}[VS Code] Installing knowledge agent...${NC}"
    mkdir -p "$CARPAI_ROOT/.vscode"
    cat > "$CARPAI_ROOT/.vscode/carpai-knowledge-agent.json" << JSONEOF
{
  "name": "$PLUGIN_NAME",
  "description": "Codebase knowledge graph and guided tour",
  "version": "1.0.0",
  "contributes": {
    "commands": [
      { "command": "carpai.understand", "title": "CarpAI: Understand Codebase" },
      { "command": "carpai.understandDomain", "title": "CarpAI: Analyze Business Domains" },
      { "command": "carpai.understandTour", "title": "CarpAI: Generate Guided Tour" },
      { "command": "carpai.understandDiff", "title": "CarpAI: Analyze Changes" }
    ]
  }
}
JSONEOF
    echo -e "${GREEN}  ✅ Plugin registered for VS Code${NC}"
}

install_for_generic() {
    echo -e "${GREEN}[Generic] Installing knowledge agent...${NC}"
    mkdir -p "$CARPAI_ROOT/.carpai/plugins"
    cat > "$CARPAI_ROOT/.carpai/plugins/$PLUGIN_NAME.toml" << TOMLEOF
name = "$PLUGIN_NAME"
description = "Codebase knowledge graph and guided tour"
version = "1.0.0"
command = "jcode understand"

[hooks]
post-commit = "jcode understand --incremental"
TOMLEOF
    echo -e "${GREEN}  ✅ Plugin registered for CarpAI native${NC}"
}

# post-commit hook
install_post_commit_hook() {
    local hook_file="$CARPAI_ROOT/.git/hooks/post-commit"
    if [ -d "$CARPAI_ROOT/.git/hooks" ]; then
        if [ ! -f "$hook_file" ] || ! grep -q "understand" "$hook_file" 2>/dev/null; then
            cat >> "$hook_file" << 'HOOKEOF'
# CarpAI: Auto-update knowledge graph on commit
if command -v jcode &>/dev/null; then
    jcode understand --incremental --quiet 2>/dev/null &
fi
HOOKEOF
            chmod +x "$hook_file" 2>/dev/null || true
            echo -e "${GREEN}  ✅ post-commit hook installed${NC}"
        else
            echo -e "${YELLOW}  ⚠️  post-commit hook already contains understand command${NC}"
        fi
    fi
}

# 主安装流程
case "$PLATFORM" in
    claude-code)
        install_for_claude_code
        ;;
    cursor)
        install_for_cursor
        ;;
    vscode)
        install_for_vscode
        ;;
    codex|opencode|gemini-cli|hermes|cline|kimi)
        install_for_generic
        echo -e "${YELLOW}  ℹ️  Platform '$PLATFORM' uses generic config${NC}"
        ;;
    unknown|*)
        echo -e "${YELLOW}  ⚠️  Unknown platform, installing for all known formats${NC}"
        install_for_claude_code
        install_for_cursor
        install_for_vscode
        install_for_generic
        ;;
esac

install_post_commit_hook

# 验证安装
echo ""
echo -e "${BLUE}━━━ Verification ━━━${NC}"
if command -v jcode &>/dev/null; then
    echo -e "${GREEN}✅ jcode CLI found: $(which jcode)${NC}"
    echo -e "${GREEN}✅ Run 'jcode understand --help' to get started${NC}"
else
    echo -e "${RED}❌ jcode CLI not found in PATH${NC}"
    echo -e "${YELLOW}  Please ensure CarpAI is installed and jcode is in your PATH${NC}"
fi

# 支持的平台列表
echo ""
echo -e "${BLUE}Supported platforms (14+):${NC}"
echo "  claude-code  cursor  codex  opencode  gemini-cli"
echo "  vscode       copilot hermes cline     kimi"
echo "  pi           antigravity  vibe  (and more...)"
echo ""
echo -e "${GREEN}✅ Installation complete!${NC}"
echo -e "${YELLOW}Tip: Run 'jcode understand' to analyze your project's codebase${NC}"
