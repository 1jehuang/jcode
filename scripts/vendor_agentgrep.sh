#!/bin/bash
# ============================================================
# agentgrep 依赖 Vendor 脚本
# 将 git 依赖 vendor 到 crates/vendor-agentgrep/
# 消除供应链风险
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
VENDOR_DIR="$PROJECT_DIR/crates/vendor-agentgrep"
AGENTGREP_REPO="https://github.com/1jehuang/agentgrep.git"
AGENTGREP_TAG="v0.1.2"

echo "============================================"
echo " agentgrep — Vendor 依赖"
echo " 源: $AGENTGREP_REPO (tag: $AGENTGREP_TAG)"
echo " 目标: $VENDOR_DIR"
echo "============================================"

# ── Step 1: 检查是否已 vendor ──
if [[ -d "$VENDOR_DIR" ]] && [[ -f "$VENDOR_DIR/Cargo.toml" ]]; then
    echo ""
    echo "[1/3] 目标目录已存在，检查版本..."
    if grep -q "$AGENTGREP_TAG" "$VENDOR_DIR/Cargo.toml" 2>/dev/null; then
        echo "  ✅ 已是最新版本 ($AGENTGREP_TAG)"
        echo "  如需重新 vendor: rm -rf $VENDOR_DIR && bash $0"
        exit 0
    fi
    echo "  ⚠️  版本不匹配，重新 vendor..."
    rm -rf "$VENDOR_DIR"
fi

# ── Step 2: Clone 并复制 ──
echo ""
echo "[1/3] 克隆 $AGENTGREP_REPO (tag: $AGENTGREP_TAG)..."
TMPDIR=$(mktemp -d)
git clone --depth 1 --branch "$AGENTGREP_TAG" "$AGENTGREP_REPO" "$TMPDIR/agentgrep" 2>&1

echo ""
echo "[2/3] 复制到 $VENDOR_DIR..."
mkdir -p "$VENDOR_DIR"
cp -r "$TMPDIR/agentgrep/"* "$VENDOR_DIR/"
rm -rf "$TMPDIR"

# 清理: 删除 .git 和 CI 文件
rm -rf "$VENDOR_DIR/.git"

echo ""
echo "[3/3] 验证..."
if [[ -f "$VENDOR_DIR/Cargo.toml" ]]; then
    echo "  ✅ Vendor 成功"
    echo "  包名: $(head -5 $VENDOR_DIR/Cargo.toml | grep name | head -1)"
    echo "  文件: $(find $VENDOR_DIR -name '*.rs' | wc -l) .rs 文件"
else
    echo "  ❌ Vendor 失败: Cargo.toml 未找到"
    exit 1
fi

echo ""
echo "============================================"
echo " 完成! 运行以下命令启用 vendor 版本:"
echo ""
echo "  # 编辑 Cargo.toml 第 291 行，将:"
echo "  agentgrep = { git = \"$AGENTGREP_REPO\", tag = \"$AGENTGREP_TAG\" }"
echo "  # 改为:"
echo "  agentgrep = { path = \"crates/vendor-agentgrep\" }"
echo "============================================"
