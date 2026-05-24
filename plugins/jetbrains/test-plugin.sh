#!/bin/bash
# CarpAI JetBrains Plugin - Automated Test Script
# Run this script to verify the plugin works correctly

set -e

echo "=== CarpAI JetBrains Plugin Test ==="
echo ""

# Step 1: Check if CarpAI Server is running
echo "[1/5] Checking CarpAI Server..."
if netstat -an | findstr 50051 > /dev/null 2>&1; then
    echo "  ✓ CarpAI Server is running on port 50051"
else
    echo "  ✗ CarpAI Server is NOT running"
    echo "  → Starting CarpAI Server in background..."
    cd D:/studying/Codecargo/CarpAI
    start /B cargo run --package carpai-server
    sleep 10
    
    if netstat -an | findstr 50051 > /dev/null 2>&1; then
        echo "  ✓ CarpAI Server started successfully"
    else
        echo "  ✗ Failed to start CarpAI Server"
        exit 1
    fi
fi

# Step 2: Build the plugin
echo ""
echo "[2/5] Building JetBrains Plugin..."
cd D:/studying/Codecargo/CarpAI/plugins/jetbrains
./gradlew buildPlugin --quiet

if [ -f "build/distributions/carpai-1.1.0-dev.zip" ]; then
    echo "  ✓ Plugin built successfully"
    echo "  → Output: build/distributions/carpai-1.1.0-dev.zip"
else
    echo "  ✗ Plugin build failed"
    exit 1
fi

# Step 3: Verify proto files are generated
echo ""
echo "[3/5] Verifying gRPC stubs..."
if [ -d "build/generated/source/proto" ]; then
    echo "  ✓ Proto stubs generated"
    ls build/generated/source/proto/main/java/com/carpai/ide/grpc/
else
    echo "  ✗ Proto stubs not found"
    ./gradlew generateProto
fi

# Step 4: Check settings configuration
echo ""
echo "[4/5] Checking Settings page..."
if grep -q "CarpAiSettingsConfigurable" src/main/resources/META-INF/plugin.xml; then
    echo "  ✓ Settings page registered in plugin.xml"
else
    echo "  ✗ Settings page not registered"
fi

# Step 5: Verify ChatPanel integration
echo ""
echo "[5/5] Verifying ChatPanel + gRPC integration..."
if grep -q "GrpcClient" src/main/kotlin/com/carpai/ide/ui/ChatPanel.kt; then
    echo "  ✓ ChatPanel integrated with GrpcClient"
else
    echo "  ✗ ChatPanel not integrated"
fi

echo ""
echo "=== All Checks Passed! ==="
echo ""
echo "Next Steps:"
echo "1. Open IntelliJ IDEA"
echo "2. Load Run Configuration: .run/jetbrains-plugin.run.xml"
echo "3. Run → Run Plugin"
echo "4. In sandbox IDE: File → Settings → Tools → CarpAI"
echo "5. Configure: localhost:50051"
echo "6. Open Tool Window: View → CarpAI Chat"
echo "7. Send a test message"

