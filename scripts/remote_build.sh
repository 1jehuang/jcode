#!/usr/bin/env bash
# Remote cargo runner (build/test/check/clippy) via SSH + rsync.
#
# Defaults:
# - Host: desktop (override with JCODE_REMOTE_HOST or --host)
# - Remote dir: .cache/remote-builds/jcode/<repo-name> (override with JCODE_REMOTE_DIR or --remote-dir)
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
  --host HOST          Remote SSH host (default: $JCODE_REMOTE_HOST or desktop)
  --remote-dir DIR     Remote project directory (default: $JCODE_REMOTE_DIR or .cache/remote-builds/jcode/<repo-name>)
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

LOCAL_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_NAME="$(basename "$LOCAL_DIR")"
REMOTE="${JCODE_REMOTE_HOST:-desktop}"
REMOTE_DIR="${JCODE_REMOTE_DIR:-.cache/remote-builds/jcode/${REPO_NAME}}"
SSH_BIN="${JCODE_REMOTE_SSH_BIN:-ssh}"
RSYNC_BIN="${JCODE_REMOTE_RSYNC_BIN:-rsync}"

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

if [[ "$REMOTE_DIR" == *" "* ]]; then
    echo "error: remote dir cannot contain spaces: $REMOTE_DIR" >&2
    exit 2
fi

for bin in "$SSH_BIN" "$RSYNC_BIN"; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        echo "error: required binary not found: $bin" >&2
        exit 2
    fi
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
    "$SSH_BIN" "$REMOTE" "$(printf 'mkdir -p %q' "$REMOTE_DIR")"
    "$RSYNC_BIN" -avz --delete \
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
printf -v REMOTE_RUN_CMD 'cd %q && %s' "$REMOTE_DIR" "$REMOTE_CARGO_CMD"
echo ""
echo "[2/3] Running on remote..."
"$SSH_BIN" "$REMOTE" "$REMOTE_RUN_CMD 2>&1"

echo ""
if [[ "$sync_back" -eq 1 ]]; then
    printf -v REMOTE_TEST_CMD 'test -f %q' "$REMOTE_DIR/$BINARY_PATH"
    if "$SSH_BIN" "$REMOTE" "$REMOTE_TEST_CMD"; then
        echo "[3/3] Syncing built artifact back..."
        mkdir -p "$(dirname "$LOCAL_DIR/$BINARY_PATH")"
        "$RSYNC_BIN" -avz "$REMOTE:$REMOTE_DIR/$BINARY_PATH" "$LOCAL_DIR/$BINARY_PATH"
        echo ""
        echo "=== Remote cargo complete ==="
        ls -la "$LOCAL_DIR/$BINARY_PATH"
    else
        echo "[3/3] Skipping sync-back: $BINARY_PATH not found on remote"
    fi
else
    echo "[3/3] Skipping binary sync-back"
fi
