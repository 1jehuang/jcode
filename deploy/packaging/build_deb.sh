#!/bin/bash
# ============================================================
# JCode — .deb 包构建脚本 (Debian/Ubuntu/KylinOS)
#
# 用法: bash deploy/packaging/build_deb.sh [version]
#       默认版本从 Cargo.toml 读取
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# 解析版本
VERSION="${1:-$(grep '^version = ' "$PROJECT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')}"
DEB_DIR="$PROJECT_DIR/target/debian/jcode_${VERSION}_amd64"
DEB_FILE="$PROJECT_DIR/target/debian/jcode_${VERSION}_amd64.deb"
ARCH="amd64"

echo "============================================"
echo " 构建 .deb 包"
echo "  版本: $VERSION"
echo "  架构: $ARCH"
echo "============================================"

# ── 1. 构建二进制 ──
echo ""
echo "[1/6] 编译 release 二进制..."
cd "$PROJECT_DIR"
cargo build --release --bin jcode --bin jcode-server --bin jcode-grpc

# ── 2. 创建 DEB 目录结构 ──
echo ""
echo "[2/6] 创建包目录结构..."
rm -rf "$DEB_DIR"
mkdir -p "$DEB_DIR/DEBIAN"
mkdir -p "$DEB_DIR/usr/local/bin"
mkdir -p "$DEB_DIR/usr/lib/systemd/system"
mkdir -p "$DEB_DIR/usr/share/jcode"
mkdir -p "$DEB_DIR/usr/share/applications"
mkdir -p "$DEB_DIR/usr/share/doc/jcode"
mkdir -p "$DEB_DIR/etc/jcode"
mkdir -p "$DEB_DIR/var/lib/jcode"
mkdir -p "$DEB_DIR/var/log/jcode"

# ── 3. 复制文件 ──
echo ""
echo "[3/6] 复制文件..."
cp target/release/jcode "$DEB_DIR/usr/local/bin/"
cp target/release/jcode-server "$DEB_DIR/usr/local/bin/"
cp target/release/jcode-grpc "$DEB_DIR/usr/local/bin/"
cp "$PROJECT_DIR/deploy/systemd/jcode-server.service" "$DEB_DIR/usr/lib/systemd/system/"
cp "$PROJECT_DIR/packaging/linux/jcode-desktop.desktop" "$DEB_DIR/usr/share/applications/"
cp "$PROJECT_DIR/README.md" "$DEB_DIR/usr/share/doc/jcode/"
cp "$PROJECT_DIR/LICENSE" "$DEB_DIR/usr/share/doc/jcode/" 2>/dev/null || true

# ── 4. 编写 DEBIAN/control ──
echo ""
echo "[4/6] 编写 DEBIAN/control..."
cat > "$DEB_DIR/DEBIAN/control" << EOF
Package: jcode
Version: $VERSION
Section: devel
Priority: optional
Architecture: $ARCH
Depends: libc6 (>= 2.28), openssl (>= 1.1)
Maintainer: JCode Contributors <jcode@example.com>
Description: AI-powered development agent
 JCode is a blazing-fast coding agent with TUI, multi-model support,
 swarm coordination, and 30+ tools. This package provides the
 jcode CLI, jcode-server (multi-protocol), and jcode-grpc binaries.
Homepage: https://github.com/1jehuang/jcode
EOF

cat > "$DEB_DIR/DEBIAN/conffiles" << EOF
/etc/jcode/config.toml
EOF

cat > "$DEB_DIR/DEBIAN/postinst" << EOF
#!/bin/bash
set -e

# 创建 jcode 用户
id -u jcode &>/dev/null || useradd --system --no-create-home --shell /usr/sbin/nologin jcode

# 设置权限
chown -R jcode:jcode /var/lib/jcode /var/log/jcode

# 配置目录
if [[ ! -f /etc/jcode/config.toml ]]; then
    cat > /etc/jcode/config.toml << 'CONFIG'
[grpc]
port = 50051
bind_addr = "0.0.0.0"
CONFIG
    chown jcode:jcode /etc/jcode/config.toml
fi

# 启用 systemd 服务
systemctl daemon-reload
systemctl enable jcode-server 2>/dev/null || true

echo "✅ JCode $VERSION 安装完成!"
echo "运行: sudo systemctl start jcode-server"
echo "查看: jcode --help"
EOF
chmod 755 "$DEB_DIR/DEBIAN/postinst"

# ── 5. 构建 .deb ──
echo ""
echo "[5/6] 构建 .deb 包..."
cd "$PROJECT_DIR/target/debian"
dpkg-deb --build "$DEB_DIR"

# ── 6. 验证 ──
echo ""
echo "[6/6] 验证..."
if [[ -f "$DEB_FILE" ]]; then
    echo "✅ .deb 包构建成功:"
    echo "   $DEB_FILE"
    echo "  大小: $(du -h "$DEB_FILE" | cut -f1)"
    echo ""
    echo "安装命令:"
    echo "  sudo dpkg -i $DEB_FILE"
    echo "  sudo apt-get install -f  # 安装依赖"
else
    echo "❌ 构建失败"
    exit 1
fi
