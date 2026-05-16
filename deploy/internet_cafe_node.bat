@echo off
REM ============================================
REM 网吧节点自动注册脚本 — 通过 Windows 计划任务运行
REM ============================================
REM 部署方式:
REM   1. 将本脚本复制到网吧电脑
REM   2. 配置服务器地址
REM   3. 设置计划任务: 计算机启动时运行本脚本
REM ============================================

set CARPAI_SERVER=http://192.168.1.100:8000
set CARPAI_NODE_NAME=网吧_01
set CARPAI_NODE_PORT=8002

REM 检查是否在营业时间（网吧通常 8:00-24:00 营业）
REM 只有在非营业时间才启动推理服务
set HOUR=%TIME:~0,2%
if "%HOUR:~0,1%"==" " set HOUR=0%HOUR:~1,1%

REM 网吧非营业时间: 0:00-7:59（此时闲置资源最多）
if %HOUR% GEQ 8 (
    echo 营业时间，不启动推理服务
    exit /b 0
)

echo 非营业时间，启动推理节点服务...
echo 注册到服务器: %CARPAI_SERVER%

REM 启动节点代理（后台运行）
start /B "CarpAI Node Agent" .\target\release\carpai-node-agent.exe
