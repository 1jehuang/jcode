#!/bin/bash
# ============================================================
# Vendor 依赖检查脚本
# 在 CI 和本地开发中确保所有 git 依赖已 vendor
# ============================================================
set -euo pipefail

MISSING=0

check_vendor() {
    local name="$1"
    local path="$2"
    
    if [[ -d "$path" ]] && [[ -f "$path/Cargo.toml" ]]; then
        echo "[OK] $name → $path"
    else
        echo "[MISSING] $name → $path (运行 scripts/vendor_agentgrep.sh)"
        MISSING=1
    fi
}

echo "Vendor 依赖检查:"
echo "============================================"

check_vendor "agentgrep" "crates/vendor-agentgrep"

echo "============================================"

if [[ $MISSING -ne 0 ]]; then
    echo "❌ 存在缺失的 vendor 依赖，请运行 scripts/vendor_agentgrep.sh"
    exit 1
else
    echo "✅ 所有 vendor 依赖已就绪"
fi
