#!/bin/bash
# ============================================================
# KylinOS V10 兼容性测试脚本
# 在 银河麒麟 V10 SP1/SP3 虚拟机上运行
#
# 用法:
#   在 KylinOS 虚拟机中执行:
#     bash packaging/kylin/test_compatibility.sh
#
# 前置条件:
#   - Rust 工具链已安装 (rustup)
#   - sudo 权限 (用于安装系统依赖)
# ============================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOG_FILE="/tmp/jcode-kylin-test-$(date +%Y%m%d-%H%M%S).log"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

PASS=0
FAIL=0
SKIP=0

log()   { echo -e "${BLUE}[$(date +%H:%M:%S)]${NC} $*" | tee -a "$LOG_FILE"; }
pass()  { echo -e "${GREEN}[PASS]${NC} $*" | tee -a "$LOG_FILE"; ((PASS++)); }
fail()  { echo -e "${RED}[FAIL]${NC} $*" | tee -a "$LOG_FILE"; ((FAIL++)); }
skip()  { echo -e "${YELLOW}[SKIP]${NC} $*" | tee -a "$LOG_FILE"; ((SKIP++)); }

check_cmd() {
    if command -v "$1" &>/dev/null; then
        pass "Command '$1' available: $(which "$1")"
        return 0
    else
        fail "Command '$1' NOT available"
        return 1
    fi
}

echo "============================================"
echo " KylinOS V10 — JCode 兼容性测试"
echo " 时间: $(date)"
echo " 系统: $(uname -a)"
echo " 发行版: $(cat /etc/os-release 2>/dev/null | head -5 || echo 'unknown')"
echo "============================================"
echo " 日志: $LOG_FILE"
echo "============================================"

# ── Phase 1: 系统信息 ──
echo -e "\n${BLUE}═══ Phase 1: 系统环境══════════════════════${NC}"

log "CPU: $(uname -m)"
log "内核: $(uname -r)"
log "glibc: $(ldd --version 2>&1 | head -1 || echo 'unknown')"
log "发行版信息:"
cat /etc/os-release 2>/dev/null | tee -a "$LOG_FILE" || echo "(无 /etc/os-release)"

# 终端信息
log "终端: ${TERM:-unknown}"
log "终端颜色: $(tput colors 2>/dev/null || echo 'unknown')"
log "Locale: ${LANG:-unknown}"

# ── Phase 2: Rust 工具链 ──
echo -e "\n${BLUE}═══ Phase 2: Rust 工具链═══════════════════${NC}"

if command -v rustc &>/dev/null; then
    pass "rustc available: $(rustc --version)"
else
    fail "rustc NOT available — run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
fi

if command -v cargo &>/dev/null; then
    pass "cargo available: $(cargo --version)"
else
    fail "cargo NOT available"
fi

# 检查 Rust 工具链目标
log "Rustup targets:"
rustup target list --installed 2>/dev/null | tee -a "$LOG_FILE" || echo "(rustup not found)"

# ── Phase 3: 系统依赖 ──
echo -e "\n${BLUE}═══ Phase 3: 系统依赖══════════════════════${NC}"

check_cmd "pkg-config"
check_cmd "gcc"
check_cmd "openssl"

# 检查关键库
for lib in "libssl-dev" "libfontconfig1-dev" "libdbus-1-dev"; do
    if dpkg -l "$lib" &>/dev/null 2>&1; then
        pass "Package '$lib' installed"
    else
        skip "Package '$lib' not installed (optional for some features)"
    fi
done

# ── Phase 4: cargo check (编译检查) ──
echo -e "\n${BLUE}═══ Phase 4: 编译检查 ═════════════════════${NC}"

if cd "$PROJECT_DIR"; then
    log "项目目录: $PROJECT_DIR"

    log "Running: cargo check (仅检查，不生成二进制)..."
    if cargo check 2>&1 | tee -a "$LOG_FILE"; then
        pass "cargo check 通过"
    else
        fail "cargo check 失败 — 请检查编译错误"
    fi

    log "Running: cargo check --no-default-features (最小构建)..."
    if cargo check --no-default-features 2>&1 | tee -a "$LOG_FILE"; then
        pass "cargo check --no-default-features 通过"
    else
        fail "cargo check --no-default-features 失败"
    fi
else
    fail "无法进入项目目录: $PROJECT_DIR"
fi

# ── Phase 5: cargo test (库测试) ──
echo -e "\n${BLUE}═══ Phase 5: 库测试  ═════════════════════${NC}"

if cd "$PROJECT_DIR"; then
    log "Running: cargo test --lib (库单元测试)..."
    if cargo test --lib 2>&1 | tail -20 | tee -a "$LOG_FILE"; then
        pass "库测试通过"
    else
        fail "库测试失败"
    fi

    log "Running: cargo test --package jcode-tui-core (TUI 核心测试)..."
    if cargo test --package jcode-tui-core 2>&1 | tail -20 | tee -a "$LOG_FILE"; then
        pass "TUI 核心测试通过"
    else
        fail "TUI 核心测试失败"
    fi
fi

# ── Phase 6: 终端兼容性测试 ──
echo -e "\n${BLUE}═══ Phase 6: 终端兼容性 ══════════════════${NC}"

# 256 色测试
if [[ $(tput colors 2>/dev/null || echo "0") -ge 256 ]]; then
    pass "终端支持 256 色 ($(tput colors) colors)"
else
    fail "终端不支持 256 色 — 可能影响 TUI 渲染"
fi

# Unicode 渲染测试
echo -e "\n终端 Unicode 渲染测试:" | tee -a "$LOG_FILE"
echo "  ┌──────────────────────────────┐" | tee -a "$LOG_FILE"
echo "  │  你好，世界！                │" | tee -a "$LOG_FILE"
echo "  │  ← → ↑ ↓  ➜  ✅ ❌ ⚠️  🚀  │" | tee -a "$LOG_FILE"
echo "  │  あいうえお カタカナ         │" | tee -a "$LOG_FILE"
echo "  │  中文汉字 こんにちは         │" | tee -a "$LOG_FILE"
echo "  └──────────────────────────────┘" | tee -a "$LOG_FILE"
echo ""
log "请检查上方 Unicode 字符是否显示正确（无乱码/方块）"

# 响应式: 询问用户
echo -e "\n${YELLOW}⚠️  请检查上方的 Unicode 渲染是否正常?${NC}"
echo -n " [y/N] "
read -r UNICODE_OK
if [[ "$UNICODE_OK" == "y" || "$UNICODE_OK" == "Y" ]]; then
    pass "Unicode 渲染正常"
else
    fail "Unicode 渲染异常 — 请检查 locale 设置 (LANG=$LANG)"
fi

# ── Phase 7: cargo build --release ──
echo -e "\n${BLUE}═══ Phase 7: Release 构建══════════════════${NC}"

if cd "$PROJECT_DIR"; then
    log "Running: cargo build --release (正式构建，耗时较长)..."
    log "开始时间: $(date)"
    
    if cargo build --release 2>&1 | tee -a "$LOG_FILE"; then
        pass "Release 构建成功"
        BUILD_OK=true
    else
        fail "Release 构建失败"
        BUILD_OK=false
    fi
    
    log "结束时间: $(date)"
fi

# ── Phase 8: TUI 冒烟测试 ──
echo -e "\n${BLUE}═══ Phase 8: TUI 冒烟测试══════════════════${NC}"

if [[ "$BUILD_OK" == true ]]; then
    log "运行: cargo run -- --help"
    if cargo run -- --help 2>&1 | head -30 | tee -a "$LOG_FILE"; then
        pass "CLI 帮助信息正常显示"
    else
        fail "CLI 帮助信息异常"
    fi

    # 测试 --version
    log "运行: cargo run -- --version"
    if cargo run -- --version 2>&1 | tee -a "$LOG_FILE"; then
        pass "版本信息正常"
    else
        fail "版本信息异常"
    fi

    # 测试 gRPC server 启动（后台运行 3 秒）
    log "测试: gRPC server 启动..."
    timeout 3 cargo run --bin jcode-grpc 2>&1 | tee -a "$LOG_FILE" || true
    if grep -q "Starting jcode gRPC server" "$LOG_FILE"; then
        pass "gRPC Server 启动正常"
    else
        fail "gRPC Server 启动异常"
    fi
else
    skip "Release 构建失败，跳过冒烟测试"
fi

# ── 总结 ──
echo ""
echo "============================================"
echo -e " 测试完成: ${GREEN}${PASS} PASS${NC} / ${RED}${FAIL} FAIL${NC} / ${YELLOW}${SKIP} SKIP${NC}"
echo " 日志文件: $LOG_FILE"
echo "============================================"

if [[ $FAIL -eq 0 ]]; then
    echo -e "${GREEN}✅ KylinOS V10 兼容性测试通过${NC}"
    echo ""
    echo "后续建议:"
    echo "  1. 运行 E2E 测试: cargo test --test e2e"
    echo "  2. 运行 TUI 状态模型测试: cargo test --test tui_state_model"
    echo "  3. 性能基准: cargo bench"
    exit 0
else
    echo -e "${RED}❌ 发现有 $FAIL 项失败，请检查日志: $LOG_FILE${NC}"
    exit 1
fi
