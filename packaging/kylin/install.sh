#!/bin/bash
# ============================================================
# JCode — KylinOS V10 安装脚本
# ============================================================
set -euo pipefail

INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="${HOME}/.jcode"

echo "============================================"
echo " JCode — KylinOS V10 安装"
echo "============================================"

# ── 检查 Root 权限 ──
if [[ $EUID -ne 0 ]]; then
    echo "❌ 需要 root 权限: sudo bash install.sh"
    exit 1
fi

# ── 复制二进制 ──
echo ""
echo "[1/4] 安装二进制文件..."
for bin in jcode jcode-server jcode-grpc; do
    if [[ -f "$bin" ]]; then
        cp "$bin" "$INSTALL_DIR/"
        chmod 755 "$INSTALL_DIR/$bin"
        echo "  ✅ $INSTALL_DIR/$bin"
    else
        echo "  ⚠️  未找到 $bin，跳过"
    fi
done

# ── 创建配置目录 ──
echo ""
echo "[2/4] 创建配置目录..."
mkdir -p "$CONFIG_DIR"
echo "  ✅ $CONFIG_DIR"

# ── 安装 systemd 服务 ──
echo ""
echo "[3/4] 安装 systemd 服务..."
SERVICE_DIR="/etc/systemd/system"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -f "$SCRIPT_DIR/jcode-server.service" ]]; then
    cp "$SCRIPT_DIR/jcode-server.service" "$SERVICE_DIR/"
    chmod 644 "$SERVICE_DIR/jcode-server.service"
    systemctl daemon-reload
    echo "  ✅ jcode-server.service 已安装"
    echo "  启动: sudo systemctl start jcode-server"
    echo "  启用: sudo systemctl enable jcode-server"
else
    echo "  ⚠️  未找到 service 文件，跳过"
fi

# ── 验证安装 ──
echo ""
echo "[4/4] 验证安装..."
for bin in jcode jcode-server jcode-grpc; do
    if command -v "$bin" &>/dev/null; then
        version=$("$bin" --version 2>/dev/null || echo "N/A")
        echo "  ✅ $bin — $version"
    fi
done

echo ""
echo "============================================"
echo " 安装完成!"
echo "============================================"
echo ""
echo "快速开始:"
echo "  jcode --help        # 查看帮助"
echo "  jcode               # 启动 TUI"
echo "  jcode-server        # 启动多协议服务"
echo ""
echo "服务管理:"
echo "  sudo systemctl status jcode-server"
echo "  sudo journalctl -u jcode-server -f"
echo ""
echo "配置: $CONFIG_DIR/config.toml"
