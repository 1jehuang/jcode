#!/bin/bash
# Remote build script - compiles on desktop via Tailscale SSH
# Usage: ./scripts/remote_build.sh [--release]

set -e

REMOTE="desktop-tailscale"
REMOTE_DIR="~/jcode"
LOCAL_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Parse args
RELEASE=""
CARGO_ARGS=""
if [[ "$1" == "--release" ]]; then
    RELEASE="--release"
    CARGO_ARGS="--release"
    BINARY_PATH="target/release/jcode"
else
    BINARY_PATH="target/debug/jcode"
fi

echo "=== Remote Build on $REMOTE ==="
echo "Local:  $LOCAL_DIR"
echo "Remote: $REMOTE_DIR"
echo "Mode:   ${RELEASE:-debug}"

# Step 1: Sync source files to remote (excluding target/, .git/)
echo ""
echo "[1/3] Syncing source files..."
rsync -avz --delete \
    --exclude 'target/' \
    --exclude '.git/' \
    --exclude '*.log' \
    --exclude '.claude/' \
    "$LOCAL_DIR/" "$REMOTE:$REMOTE_DIR/"

# Step 2: Build on remote
echo ""
echo "[2/3] Building on remote..."
ssh "$REMOTE" "cd $REMOTE_DIR && cargo build $CARGO_ARGS 2>&1"

# Step 3: Sync binary back
echo ""
echo "[3/3] Syncing binary back..."
rsync -avz "$REMOTE:$REMOTE_DIR/$BINARY_PATH" "$LOCAL_DIR/$BINARY_PATH"

echo ""
echo "=== Build complete ==="
ls -la "$LOCAL_DIR/$BINARY_PATH"
