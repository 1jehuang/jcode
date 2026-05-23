<#
.SYNOPSIS
    Carpvoid 边缘节点一键安装器
.DESCRIPTION
    在网吧电脑/个人笔记本上安装 CarpAI 分布式推理客户端
    自动检测硬件 → 选择合适的模型 → 注册到协调器 → 开始挖算力
.PARAMETER Coordinator
    协调器地址 (默认: http://carpai.example.com:50051)
.PARAMETER ModelDir
    模型存储目录 (默认: ~/.carpvoid/models/)
.PARAMETER NoGPU
    强制使用 CPU 模式
.EXAMPLE
    .\carpvoid-installer.ps1
    .\carpvoid-installer.ps1 -Coordinator "http://192.168.1.100:50051"
#>

param(
    [string]$Coordinator = "http://carpai.example.com:50051",
    [string]$ModelDir = "$env:USERPROFILE\.carpvoid\models",
    [switch]$NoGPU = $false
)

$VERSION = "0.1.0"
$HOST = hostname

# ─── 颜色 ───
$GREEN = "Green"
$YELLOW = "Yellow"
$RED = "Red"
$CYAN = "Cyan"

Write-Host "━━━ Carpvoid Client v$VERSION Installer ━━━" -ForegroundColor $CYAN
Write-Host ""

# ─── 1. 检测硬件 ───
Write-Host "[1/5] 检测硬件..." -ForegroundColor $CYAN

$GPUName = "无独显"
$VRAM = 0
$CPUCores = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
$RAM = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 0)

# 检测 GPU
try {
    $gpu = Get-CimInstance Win32_VideoController | Select-Object -First 1
    $GPUName = $gpu.Name
    $VRAM = [math]::Round($gpu.AdapterRAM / 1GB, 0)
    Write-Host "  GPU: $GPUName ($VRAM GB)" -ForegroundColor $GREEN
} catch {
    Write-Host "  GPU: 未检测到 (将使用 CPU 模式)" -ForegroundColor $YELLOW
}
Write-Host "  CPU: $CPUCores cores" -ForegroundColor $GREEN
Write-Host "  RAM: $RAM GB" -ForegroundColor $GREEN

# ─── 2. 选择模型 ───
Write-Host ""
Write-Host "[2/5] 选择推理模型..." -ForegroundColor $CYAN

if ($NoGPU -or $VRAM -eq 0) {
    $ModelName = "Qwen3-1.5B-Q4_0"
    $ModelSize = "~1 GB"
    $MinRAM = 4
} elseif ($VRAM -ge 12) {
    $ModelName = "Qwen3-14B-Q4_K_M"
    $ModelSize = "~8 GB"
    $MinRAM = 16
} elseif ($VRAM -ge 6) {
    $ModelName = "Qwen3-7B-Q4_K_M"
    $ModelSize = "~4 GB"
    $MinRAM = 8
} else {
    $ModelName = "Qwen3-1.5B-Q4_0"
    $ModelSize = "~1 GB"
    $MinRAM = 4
}

if ($RAM -lt $MinRAM) {
    Write-Host "  ⚠️  内存不足 ($RAM GB < $MinRAM GB), 使用最小模型" -ForegroundColor $YELLOW
    $ModelName = "Qwen3-1.5B-Q4_0"
    $ModelSize = "~1 GB"
}

Write-Host "  选择模型: $ModelName ($ModelSize)" -ForegroundColor $GREEN

# ─── 3. 下载模型 ───
Write-Host ""
Write-Host "[3/5] 下载模型..." -ForegroundColor $CYAN

New-Item -ItemType Directory -Force -Path $ModelDir | Out-Null
$ModelPath = Join-Path $ModelDir "$ModelName.gguf"

if (Test-Path $ModelPath) {
    Write-Host "  模型已存在: $ModelPath" -ForegroundColor $GREEN
} else {
    $ModelUrl = "https://huggingface.co/Qwen/$ModelName-GGUF/resolve/main/$ModelName.gguf"
    Write-Host "  下载中: $ModelUrl" -ForegroundColor $YELLOW
    Write-Host "  保存到: $ModelPath" -ForegroundColor $YELLOW
    try {
        Invoke-WebRequest -Uri $ModelUrl -OutFile $ModelPath -UseBasicParsing
        Write-Host "  ✅ 下载完成" -ForegroundColor $GREEN
    } catch {
        Write-Host "  ❌ 下载失败: $_" -ForegroundColor $RED
        Write-Host "  请手动下载模型到: $ModelPath" -ForegroundColor $YELLOW
    }
}

# ─── 4. 注册到协调器 ───
Write-Host ""
Write-Host "[4/5] 注册到 CarpAI 协调器..." -ForegroundColor $CYAN

$Registration = @{
    node_id = "carpvoid-$HOST-$(Get-Random -Maximum 99999)"
    node_name = $HOST
    hardware = @{
        gpu_name = $GPUName
        vram_mb = $VRAM * 1024
        ram_mb = $RAM * 1024
        cpu_cores = $CPUCores
        os = "windows"
        has_cuda = ($GPUName -match "NVIDIA")
        has_vulkan = $false
        is_laptop = ($HOST -match "LAPTOP|NOTEBOOK|BOOK")
    }
    suggested_model = $ModelName
    version = $VERSION
    capabilities = @("inference", if ($GPUName -match "NVIDIA") { "cuda" } else { "cpu" })
} | ConvertTo-Json

try {
    $Response = Invoke-RestMethod -Uri "$Coordinator/api/v1/distributed/register" `
        -Method Post `
        -Body $Registration `
        -ContentType "application/json" `
        -TimeoutSec 10
    Write-Host "  ✅ 注册成功!" -ForegroundColor $GREEN
    Write-Host "  Node ID: $($Registration.node_id)" -ForegroundColor $GREEN
} catch {
    Write-Host "  ⚠️  注册失败: $_" -ForegroundColor $YELLOW
    Write-Host "  协调器 $Coordinator 暂时不可用, 稍后重试" -ForegroundColor $YELLOW
}

# ─── 5. 创建启动脚本 ───
Write-Host ""
Write-Host "[5/5] 创建启动脚本..." -ForegroundColor $CYAN

$StartupScript = @"
@echo off
title Carpvoid Client - $HOST
echo ━━━ Carpvoid Client v$VERSION ━━━
echo Node: $HOST
echo Model: $ModelName
echo Coordinator: $Coordinator
echo.
carpvoid-client --coordinator "$Coordinator"
pause
"@

$StartupPath = Join-Path $ModelDir "..\start.bat" | Resolve-Path -ErrorAction SilentlyContinue
if (-not $StartupPath) {
    $StartupPath = Join-Path $HOME ".carpvoid\start.bat"
    New-Item -ItemType Directory -Force -Path (Split-Path $StartupPath) | Out-Null
}
$StartupScript | Out-File -FilePath $StartupPath -Encoding ascii

Write-Host "  ✅ 启动脚本: $StartupPath" -ForegroundColor $GREEN

# ─── 完成 ───
Write-Host ""
Write-Host "━━━ 安装完成! ━━━" -ForegroundColor $CYAN
Write-Host ""
Write-Host "📋 摘要" -ForegroundColor $CYAN
Write-Host "  节点:    $HOST"
Write-Host "  模型:    $ModelName ($ModelSize)"
Write-Host "  协调器:  $Coordinator"
Write-Host "  模式:    $(if ($NoGPU -or $VRAM -eq 0) { 'CPU' } else { 'GPU' })"
Write-Host ""
Write-Host "▶️  启动客户端:" -ForegroundColor $GREEN
Write-Host "  双击: $StartupPath"
Write-Host "  或:   carpvoid-client --coordinator ""$Coordinator"""
Write-Host ""
Write-Host "🪙  收益说明:" -ForegroundColor $YELLOW
Write-Host "  每完成 100 次推理 = 1 小时免费上网时长"
Write-Host "  每完成 1000 次推理 = 免费套餐一份"
Write-Host "  详情: https://carpai.dev/rewards"
Write-Host ""

# 如果是在网吧环境, 自动启动
if ($env:COMPUTERNAME -match "BAR|CAFE|NET|PUBLIC") {
    Write-Host "检测到网吧环境, 自动启动客户端..." -ForegroundColor $GREEN
    Start-Process -WindowStyle Hidden -FilePath "carpvoid-client.exe" -ArgumentList "--coordinator",$Coordinator
}

pause
