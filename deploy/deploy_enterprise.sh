#!/bin/bash
# ============================================
# CarpAI Enterprise Server — 快速部署脚本 (Linux)
# ============================================
# 运行: bash deploy/deploy_enterprise.sh
#
# 前置条件:
#   1. 已安装 Rust (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh)
#   2. 已安装 llama.cpp (git clone ... && make -j)
#   3. 已下载量化模型
# ============================================

set -e

echo "============================================"
echo " CarpAI Enterprise Server — Linux 部署"
echo "============================================"
echo ""

# 检查必需工具
for cmd in cargo git python3; do
    if ! command -v $cmd &> /dev/null; then
        echo "[错误] 未找到 $cmd，请先安装"
        exit 1
    fi
done

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"

# 创建必要目录
mkdir -p data models kv_cache_mmap logs

echo "[1/4] 编译企业版服务器..."
cargo build --release --package jcode-enterprise-server
echo ""

echo "[2/4] 检查/安装 llama.cpp..."
if ! command -v llama-server &> /dev/null; then
    echo "[安装] 正在编译 llama.cpp..."
    if [ ! -d "llama.cpp" ]; then
        git clone --depth 1 https://github.com/ggerganov/llama.cpp.git
    fi
    cd llama.cpp
    make -j$(nproc) llama-server
    cp llama-server /usr/local/bin/ 2>/dev/null || true
    cd "$SCRIPT_DIR"
fi
echo ""

echo "[3/4] 检查量化模型..."
if [ ! -f "models/qwen3-72b-Q4_K_M.gguf" ]; then
    echo "[提示] 未找到量化模型。运行以下命令下载:"
    echo "   pip install huggingface-hub"
    echo "   python3 scripts/download_quantize.py --model Qwen/Qwen3-72B --quant Q4_K_M"
    echo ""
    echo "[可选] 可先运行轻量化模型或跳过此步骤"
fi

echo "[4/4] 启动企业版服务器..."
echo ""
echo "-------------------------------------------"
echo " API:    http://localhost:8000"
echo " Admin:  http://localhost:8001"
echo " Node:   http://localhost:8002"
echo " 日志:   ./logs/server.log"
echo "-------------------------------------------"
echo ""

export CARPAI_LOG_LEVEL=info
export CARPAI_DATABASE_URL="sqlite://./data/carpai_enterprise.db?mode=rwc"
export RUST_LOG=info

nohup ./target/release/carpai-enterprise-server > logs/server.log 2>&1 &
echo "服务器已启动 (PID: $!)"
echo "查看日志: tail -f logs/server.log"
