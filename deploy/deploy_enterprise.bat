@echo off
REM ============================================
REM CarpAI Enterprise Server — 快速部署脚本 (Windows)
REM ============================================
REM 运行方式: 双击或 cmd 运行 deploy_enterprise.bat
REM
REM 前置条件:
REM   1. 已安装 Rust (https://rustup.rs)
REM   2. 已安装 llama.cpp (https://github.com/ggerganov/llama.cpp)
REM   3. 已下载量化模型 (运行 scripts/download_quantize.py)
REM ============================================

echo ============================================
echo  CarpAI Enterprise Server — Windows 部署
echo ============================================
echo.

REM 检查 Rust 是否安装
where cargo >nul 2>nul
if %ERRORLEVEL% NEQ 0 (
    echo [错误] 未找到 cargo。请先安装 Rust: https://rustup.rs
    exit /b 1
)

REM 检查 llama.cpp 是否安装
where llama-server >nul 2>nul
if %ERRORLEVEL% NEQ 0 (
    echo [警告] 未找到 llama-server。请安装 llama.cpp:
    echo   git clone https://github.com/ggerganov/llama.cpp
    echo   cd llama.cpp ^&^& mkdir build ^&^& cd build
    echo   cmake .. -DCMAKE_BUILD_TYPE=Release
    echo   cmake --build . --config Release
    echo   并将 build\bin\Release 添加到 PATH
    echo.
)

REM 创建必要目录
if not exist "data" mkdir data
if not exist "models" mkdir models
if not exist "kv_cache_mmap" mkdir kv_cache_mmap
if not exist "logs" mkdir logs

echo [1/3] 编译企业版服务器...
cd /d "%~dp0"
cargo build --release --package jcode-enterprise-server

if %ERRORLEVEL% NEQ 0 (
    echo [错误] 编译失败
    exit /b 1
)

echo [2/3] 检查量化模型...
if not exist "models\qwen3-72b-Q4_K_M.gguf" (
    echo [提示] 未找到 quantized 模型。
    echo   运行以下命令下载并量化:
    echo   pip install huggingface-hub
    echo   python scripts/download_quantize.py --model Qwen/Qwen3-72B --quant Q4_K_M
)

echo [3/3] 启动服务器...
echo.
echo -------------------------------------------
echo  API:     http://localhost:8000
echo  Admin:   http://localhost:8001
echo  Node:    http://localhost:8002
echo  日志:    ./logs/server.log
echo -------------------------------------------
echo.

set CARPAI_LOG_LEVEL=info
set CARPAI_DATABASE_URL=sqlite://./data/carpai_enterprise.db?mode=rwc

.\target\release\carpai-enterprise-server.exe
