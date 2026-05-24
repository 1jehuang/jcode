@echo off
REM CarpAI JetBrains Plugin - Windows Test Script
REM Run this from Command Prompt

echo === CarpAI JetBrains Plugin Test ===
echo.

REM Step 1: Check if CarpAI Server is running
echo [1/5] Checking CarpAI Server...
netstat -an | findstr 50051 >nul 2>&1
if %errorlevel% equ 0 (
    echo   [OK] CarpAI Server is running on port 50051
) else (
    echo   [FAIL] CarpAI Server is NOT running
    echo   Please start it manually: cargo run --package carpai-server
)

REM Step 2: Build the plugin
echo.
echo [2/5] Building JetBrains Plugin...
cd /d D:/studying/Codecargo/CarpAI/plugins/jetbrains
call gradlew.bat buildPlugin --quiet

if exist "build/distributions/carpai-1.1.0-dev.zip" (
    echo   [OK] Plugin built successfully
    echo   Output: build/distributions/carpai-1.1.0-dev.zip
) else (
    echo   [FAIL] Plugin build failed
    exit /b 1
)

REM Step 3: Verify proto files
echo.
echo [3/5] Verifying gRPC stubs...
if exist "build/generated/source/proto" (
    echo   [OK] Proto stubs generated
) else (
    echo   [FAIL] Proto stubs not found
)

echo.
echo === Checks Complete ===
echo.
echo Next Steps:
echo 1. Open IntelliJ IDEA
echo 2. Run → Run Plugin
echo 3. In sandbox IDE: Settings → Tools → CarpAI → Configure localhost:50051
echo 4. Open Tool Window: CarpAI Chat → Send test message

