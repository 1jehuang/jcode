#!/bin/bash
# CarpAI Extended Harness Runner
# Runs all 6 extended harness tests covering:
# LSP Server | AutoFallback | REST LLM | Knowledge Agents | Claude Agent Port | LSP CodeActions
#
# Usage: bash scripts/run_harness.sh [--verbose] [--test <name>]

set -euo pipefail

CARPAI_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERBOSE="${1:-}"
TEST_FILTER="${2:-}"

echo "━━━ CarpAI Extended Harness ━━━"
echo ""

run_test() {
    local name="$1"
    local filter="$2"

    if [ -n "$TEST_FILTER" ] && [ "$name" != "$TEST_FILTER" ]; then
        return
    fi

    echo "▶  $name..."

    if [ "$VERBOSE" = "--verbose" ] || [ "$VERBOSE" = "-v" ]; then
        cargo test --test extended_harness "$filter" -- --nocapture 2>&1
    else
        cargo test --test extended_harness "$filter" 2>&1 | tail -3
    fi
}

# Run all 6 harness tests
run_test "LSP Server"        "test_lsp_server"
run_test "AutoFallback"      "test_auto_fallback"
run_test "REST LLM"          "test_rest_llm"
run_test "Knowledge Agents"  "test_knowledge_agents"
run_test "Claude Agent Port" "test_claude_agent_port"
run_test "LSP CodeActions"   "test_lsp_code_actions"

echo ""
echo "━━━ Harness complete ━━━"
echo ""
echo "Quick commands:"
echo "  bash scripts/run_harness.sh                           # Run all"
echo "  bash scripts/run_harness.sh --verbose                  # With full output"
echo "  bash scripts/run_harness.sh \"\" \"test_lsp_server\"   # Single test"
echo "  cargo test --test extended_harness -- --nocapture     # Via cargo"
