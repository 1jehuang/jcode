#!/bin/bash
# ============================================================
# KylinOS V10 — 交叉编译脚本
# 在 x86_64 开发机上构建 aarch64 (鲲鹏/飞腾) 目标
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

KYLIN_ARCH="${1:-aarch64}"  # aarch64 or x86_64
RUST_TARGET=""
TRIPLE=""

case "$KYLIN_ARCH" in
    aarch64|arm64)
        RUST_TARGET="aarch64-unknown-linux-gnu"
        TRIPLE="aarch64-linux-gnu"
        ;;
    x86_64|amd64)
        RUST_TARGET="x86_64-unknown-linux-gnu"
        TRIPLE="x86_64-linux-gnu"
        ;;
    *)
        echo "用法: $0 {aarch64|x86_64}"
        exit 1
        ;;
esac

echo "============================================"
echo " JCode — KylinOS V10 交叉编译"
echo " 目标架构: $KYLIN_ARCH ($RUST_TARGET)"
echo "============================================"

# ── Step 1: 安装交叉编译工具链 ──
echo ""
echo "[1/5] 安装交叉编译工具链..."

if ! dpkg -l "gcc-$TRIPLE" &>/dev/null 2>&1; then
    echo "  -> 安装 gcc-$TRIPLE..."
    sudo apt-get install -y "gcc-$TRIPLE" "libc6-dev-$KYLIN_ARCH-cross" 2>/dev/null || {
        echo "  ⚠️  apt 安装失败，尝试使用 rustup target add"
    }
fi

# ── Step 2: 安装 Rust 目标 ──
echo ""
echo "[2/5] 安装 Rust 目标: $RUST_TARGET"
rustup target add "$RUST_TARGET"

# ── Step 3: 配置链接器 ──
echo ""
echo "[3/5] 配置链接器..."
mkdir -p "$PROJECT_DIR/.cargo"
CARGO_CONFIG="$PROJECT_DIR/.cargo/config.toml"

cat > "$CARGO_CONFIG" << EOF
# KylinOS V10 交叉编译配置
# 生成方式: packaging/kylin/cross_compile.sh

[target.$RUST_TARGET]
linker = "$TRIPLE-gcc"
EOF

echo "  -> 写入 $CARGO_CONFIG"

# ── Step 4: 构建 ──
echo ""
echo "[4/5] 开始交叉编译: cargo build --release --target $RUST_TARGET"
echo "  开始时间: $(date)"

cd "$PROJECT_DIR"
cargo build --release --target "$RUST_TARGET" 2>&1 | tee /tmp/jcode-kylin-build.log

if [ $? -eq 0 ]; then
    echo ""
    echo "[5/5] 构建成功！二进制文件:"
    echo "  target/$RUST_TARGET/release/jcode"
    echo "  target/$RUST_TARGET/release/jcode-server"
    echo "  target/$RUST_TARGET/release/jcode-grpc"

    # 检查 glibc 版本兼容性
    if command -v "$TRIPLE-readelf" &>/dev/null; then
        echo ""
        echo "  glibc 版本需求检查:"
        "$TRIPLE-readelf" -V "target/$RUST_TARGET/release/jcode" 2>/dev/null | grep -i "glibc" | head -5 || true
    fi

    # 打包
    echo ""
    echo "  打包为 tar.gz..."
    cd "$PROJECT_DIR"
    tar -czf "/tmp/jcode-kylin-$KYLIN_ARCH.tar.gz" \
        -C "target/$RUST_TARGET/release" \
        jcode jcode-server jcode-grpc \
        -C "$PROJECT_DIR/packaging/kylin" \
        install.sh README.md
    echo "  打包完成: /tmp/jcode-kylin-$KYLIN_ARCH.tar.gz"
else
    echo ""
    echo "❌ 编译失败，请检查日志: /tmp/jcode-kylin-build.log"
    exit 1
fi

# ── 恢复 cargo config ──
git checkout -- "$CARGO_CONFIG" 2>/dev/null || true
echo ""
echo "============================================"
echo " 完成! 在 KylinOS 目标机上运行:"
echo "   1. scp /tmp/jcode-kylin-$KYLIN_ARCH.tar.gz user@kylin:~"
echo "   2. 在 KylinOS 上: tar -xzf jcode-kylin-$KYLIN_ARCH.tar.gz"
echo "   3. sudo ./install.sh"
echo "   4. jcode --help"
echo "============================================"
