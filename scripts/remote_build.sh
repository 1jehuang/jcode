#!/usr/bin/env bash
# Remote cargo runner (build/test/check/clippy) via SSH + rsync.
#
# Defaults:
# - Host: desktop-tailscale (override with JCODE_REMOTE_HOST or --host)
# - Remote dir: ~/jcode (override with JCODE_REMOTE_DIR or --remote-dir)
#
# Examples:
#   scripts/remote_build.sh --release
#   scripts/remote_build.sh test
#   scripts/remote_build.sh check --all-targets
#   scripts/remote_build.sh --host mybox --remote-dir ~/src/jcode test -- --nocapture

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/remote_build.sh [options] [cargo-subcommand] [cargo-args...]

Options:
  -r, --release        Add --release to cargo invocation
  --host HOST          Remote SSH host (default: $JCODE_REMOTE_HOST or desktop-tailscale)
  --remote-dir DIR     Remote project directory (default: $JCODE_REMOTE_DIR or ~/jcode)
  --no-sync            Skip rsync upload step
  --sync-back          Force sync-back of built binary after command
  --no-sync-back       Disable sync-back of built binary after command
  -h, --help           Show this help

Behavior:
  - Default cargo subcommand is 'build'
  - Sync-back defaults to ON for 'build', OFF for other subcommands
  - For build sync-back, copies target/{debug|release}/<artifact> from remote to local
    (artifact defaults to 'jcode', or '--bin <name>' when provided)
EOF
}

REMOTE="${JCODE_REMOTE_HOST:-desktop-tailscale}"
REMOTE_DIR="${JCODE_REMOTE_DIR:-~/jcode}"
LOCAL_DIR="$(cd "$(dirname "$0")/.." && pwd)"

SYNC_SOURCE=1
SYNC_BACK_MODE="auto" # auto|always|never
RELEASE=0
SUBCOMMAND="build"
SUBCOMMAND_SET=0
POSITIONAL=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -r|--release)
            RELEASE=1
            shift
            ;;
        --host)
            [[ $# -lt 2 ]] && { echo "error: --host requires a value" >&2; exit 2; }
            REMOTE="$2"
            shift 2
            ;;
        --remote-dir)
            [[ $# -lt 2 ]] && { echo "error: --remote-dir requires a value" >&2; exit 2; }
            REMOTE_DIR="$2"
            shift 2
            ;;
        --no-sync)
            SYNC_SOURCE=0
            shift
            ;;
        --sync-back)
            SYNC_BACK_MODE="always"
            shift
            ;;
        --no-sync-back)
            SYNC_BACK_MODE="never"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            POSITIONAL+=("$@")
            break
            ;;
        *)
            if [[ "$SUBCOMMAND_SET" -eq 0 && "$1" != -* ]]; then
                SUBCOMMAND="$1"
                SUBCOMMAND_SET=1
            else
                POSITIONAL+=("$1")
            fi
            shift
            ;;
    esac
done

CARGO_CMD=(cargo "$SUBCOMMAND")
if [[ "$RELEASE" -eq 1 ]]; then
    CARGO_CMD+=(--release)
fi
if [[ "${#POSITIONAL[@]}" -gt 0 ]]; then
    CARGO_CMD+=("${POSITIONAL[@]}")
fi

sync_back=0
case "$SYNC_BACK_MODE" in
    always) sync_back=1 ;;
    never) sync_back=0 ;;
    auto)
        if [[ "$SUBCOMMAND" == "build" ]]; then
            sync_back=1
        fi
        ;;
esac

if [[ "$RELEASE" -eq 1 ]]; then
    build_mode="release"
else
    build_mode="debug"
fi

artifact_name="jcode"
if [[ "$SUBCOMMAND" == "build" ]]; then
    for ((i=0; i<${#POSITIONAL[@]}; i++)); do
        if [[ "${POSITIONAL[$i]}" == "--bin" && $((i + 1)) -lt ${#POSITIONAL[@]} ]]; then
            artifact_name="${POSITIONAL[$((i + 1))]}"
            break
        fi
    done
fi

BINARY_PATH="target/${build_mode}/${artifact_name}"

echo "=== Remote Cargo on $REMOTE ==="
echo "Local:   $LOCAL_DIR"
echo "Remote:  $REMOTE_DIR"
echo "Command: ${CARGO_CMD[*]}"
echo "Mode:    $build_mode"

if [[ "$SYNC_SOURCE" -eq 1 ]]; then
    echo ""
    echo "[1/3] Syncing source files..."
    rsync -avz --delete \
        --exclude 'target/' \
        --exclude '.git/' \
        --exclude '*.log' \
        --exclude '.claude/' \
        "$LOCAL_DIR/" "$REMOTE:$REMOTE_DIR/"
else
    echo ""
    echo "[1/3] Skipping source sync (--no-sync)"
fi

printf -v REMOTE_CARGO_CMD '%q ' "${CARGO_CMD[@]}"
echo ""
echo "[2/3] Running on remote..."
ssh "$REMOTE" "cd $REMOTE_DIR && $REMOTE_CARGO_CMD 2>&1"

echo ""
if [[ "$sync_back" -eq 1 ]]; then
    if ssh "$REMOTE" "test -f $REMOTE_DIR/$BINARY_PATH"; then
        echo "[3/3] Syncing built artifact back..."
        mkdir -p "$(dirname "$LOCAL_DIR/$BINARY_PATH")"
        rsync -avz "$REMOTE:$REMOTE_DIR/$BINARY_PATH" "$LOCAL_DIR/$BINARY_PATH"
        echo ""
        echo "=== Remote cargo complete ==="
        ls -la "$LOCAL_DIR/$BINARY_PATH"
    else
        echo "[3/3] Skipping sync-back: $BINARY_PATH not found on remote"
    fi
else
    echo "[3/3] Skipping binary sync-back"
fi
