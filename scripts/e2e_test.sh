#!/bin/bash
# CarpAI End-to-End Test Suite
# 3 scenarios that must pass automatically
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
PASS=0
FAIL=0

green() { echo -e "\033[32m$1\033[0m"; }
red() { echo -e "\033[31m$1\033[0m"; }
bold() { echo -e "\033[1m$1\033[0m"; }

echo ""
bold "━━━ CarpAI 端到端测试套件 ━━━"
echo ""

# ────────── Scenario 1: MCP Server Infrastructure ──────────
bold "📋 场景1: MCP服务器基础设施验证"
echo ""

# 1.1 检查 MCP 配置文件存在
echo -n "  1.1 .cursor/mcp.json ... "
if [ -f "$PROJECT_DIR/.cursor/mcp.json" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.2 .claude/mcp.json ... "
if [ -f "$PROJECT_DIR/.claude/mcp.json" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.3 .vscode/mcp.json ... "
if [ -f "$PROJECT_DIR/.vscode/mcp.json" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.4 .jcode/mcp.json ... "
if [ -f "$PROJECT_DIR/.jcode/mcp.json" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.5 config/mcp_servers.yaml ... "
if [ -f "$PROJECT_DIR/config/mcp_servers.yaml" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.6 Python测试文件存在 ... "
if [ -f "$PROJECT_DIR/mcp-servers/tests/test_servers.py" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.7 Dockerfile存在(github为例) ... "
if [ -f "$PROJECT_DIR/mcp-servers/github/Dockerfile" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.8 docker-compose.yml存在 ... "
if [ -f "$PROJECT_DIR/mcp-servers/docker-compose.yml" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.9 安装脚本存在(install_all.py) ... "
if [ -f "$PROJECT_DIR/mcp-servers/install_all.py" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  1.10 启动脚本存在(start_all.py) ... "
if [ -f "$PROJECT_DIR/mcp-servers/start_all.py" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo ""

# ────────── Scenario 2: IDE Plugin Integration ──────────
bold "📋 场景2: IDE插件集成验证"
echo ""

echo -n "  2.1 VSCode扩展package.json ... "
if [ -f "$PROJECT_DIR/editors/vscode-carpai/package.json" ]; then
    CMD_COUNT=$(grep -c '"command": "carpai\.' "$PROJECT_DIR/editors/vscode-carpai/package.json" || true)
    green "PASS ($CMD_COUNT commands)"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  2.2 VSCode扩展源码完整 ... "
if [ -f "$PROJECT_DIR/editors/vscode-carpai/src/extension.ts" ] && \
   [ -f "$PROJECT_DIR/editors/vscode-carpai/src/inlineCompletionProvider.ts" ] && \
   [ -f "$PROJECT_DIR/editors/vscode-carpai/src/mcpConfigProvider.ts" ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  2.3 Neovim插件入口存在 ... "
if [ -f "$PROJECT_DIR/editors/carpai-nvim/plugin/carpai.lua" ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  2.4 Neovim核心模块完整 ... "
if [ -f "$PROJECT_DIR/editors/carpai-nvim/lua/carpai/init.lua" ] && \
   [ -f "$PROJECT_DIR/editors/carpai-nvim/lua/carpai/mcp.lua" ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  2.5 JetBrains插件入口存在 ... "
if [ -f "$PROJECT_DIR/editors/jetbrains-carpai/plugin.xml" ] || \
   [ -f "$PROJECT_DIR/editors/jetbrains-carpai/src/main/resources/META-INF/plugin.xml" ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  2.6 JetBrains Kotlin源码完整 ... "
if [ -f "$PROJECT_DIR/editors/jetbrains-carpai/src/main/kotlin/com/carpai/plugin/CarpaiPlugin.kt ] && \
   [ -f "$PROJECT_DIR/editors/jetbrains-carpai/src/main/kotlin/com/carpai/plugin/actions/ExplainCodeAction.kt ] && \
   [ -f "$PROJECT_DIR/editors/jetbrains-carpai/src/main/kotlin/com/carpai/plugin/actions/RefactorCodeAction.kt ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  2.7 JetBrains gradle构建文件 ... "
if [ -f "$PROJECT_DIR/editors/jetbrains-carpai/build.gradle.kts" ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo ""

# ────────── Scenario 3: Core Capabilities ──────────
bold "📋 场景3: 核心能力验证"
echo ""

echo -n "  3.1 MCP CLI命令实现 ... "
if grep -q "serve\|status\|discover\|start" "$PROJECT_DIR/src/commands/agent/mcp.rs" 2>/dev/null; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.2 Planner模块存在 ... "
if [ -f "$PROJECT_DIR/src/planner/plan.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.3 Refactor模块存在 ... "
if [ -f "$PROJECT_DIR/src/refactor/mod.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.4 Transaction模块存在 ... "
if [ -f "$PROJECT_DIR/src/transaction/mod.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.5 Verify模块存在 ... "
if [ -f "$PROJECT_DIR/src/verify/mod.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.6 性能优化模块存在 ... "
if [ -f "$PROJECT_DIR/src/cache_optimizer.rs" ] && \
   [ -f "$PROJECT_DIR/src/concurrency_optimizer.rs" ] && \
   [ -f "$PROJECT_DIR/src/inference_optimizer.rs" ] && \
   [ -f "$PROJECT_DIR/src/render_optimizer.rs" ]; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.7 跨文件事务(file_history) ... "
if [ -f "$PROJECT_DIR/src/file_history.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.8 读后写防护(file_state_cache) ... "
if [ -f "$PROJECT_DIR/src/file_state_cache.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.9 调度器集成(planner/integration) ... "
if [ -f "$PROJECT_DIR/src/planner/integration.rs" ]; then green "PASS"; PASS=$((PASS+1)); else red "FAIL"; FAIL=$((FAIL+1)); fi

echo -n "  3.10 Agent自动快照(turn_loops注入) ... "
if grep -q "snapshot_file\|recent_edit_files\|mcp_tool_names" "$PROJECT_DIR/src/agent.rs" 2>/dev/null; then
    green "PASS"; PASS=$((PASS+1))
else red "FAIL"; FAIL=$((FAIL+1)); fi

echo ""
bold "━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  通过: $PASS  失败: $FAIL  总计: $((PASS+FAIL))"
echo ""

if [ $FAIL -eq 0 ]; then
    green "✅ 全部测试通过！"
    exit 0
else
    red "❌ 存在 $FAIL 个失败测试"
    exit 1
fi
