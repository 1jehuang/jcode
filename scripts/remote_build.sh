#!/usr/bin/env bash
# Remote cargo runner (build/test/check/clippy) via SSH + rsync.
#
# Defaults:
# - Config file: ~/.config/jcode/remote-build.env (override with JCODE_REMOTE_CONFIG)
# - Host: JCODE_REMOTE_HOST from env/config, or --host
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
  --host HOST          Remote SSH host (default: $JCODE_REMOTE_HOST from env/config; required if unset)
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
  - Default config file is ~/.config/jcode/remote-build.env
EOF
}

LOCAL_DIR="$(cd "$(dirname "$0")/.." && pwd)"
REPO_NAME="$(basename "$LOCAL_DIR")"

# shellcheck source=scripts/remote_config.sh
source "$LOCAL_DIR/scripts/remote_config.sh"
jcode_load_remote_config

REMOTE="${JCODE_REMOTE_HOST:-}"
REMOTE_DIR="${JCODE_REMOTE_DIR:-.cache/remote-builds/jcode/${REPO_NAME}}"
SSH_BIN="${JCODE_REMOTE_SSH_BIN:-ssh}"
RSYNC_BIN="${JCODE_REMOTE_RSYNC_BIN:-rsync}"

SYNC_SOURCE=1
SYNC_BACK_MODE="auto" # auto|always|never
RELEASE=0
SUBCOMMAND="build"
SUBCOMMAND_SET=0
POSITIONAL=()

remote_connect_timeout() {
    local value="${JCODE_REMOTE_CONNECT_TIMEOUT:-5}"
    if [[ ! "$value" =~ ^[0-9]+$ || "$value" -lt 1 ]]; then
        value=5
    fi
    printf '%s\n' "$value"
}

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

if [[ -z "$REMOTE" ]]; then
    echo "error: remote host not configured; set JCODE_REMOTE_HOST or pass --host HOST" >&2
    exit 2
fi

for bin in "$SSH_BIN" "$RSYNC_BIN"; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        echo "error: required binary not found: $bin" >&2
        exit 2
    fi
done

SSH_CONNECT_TIMEOUT="$(remote_connect_timeout)"
SSH_SERVER_ALIVE_INTERVAL="${JCODE_REMOTE_SERVER_ALIVE_INTERVAL:-10}"
SSH_SERVER_ALIVE_COUNT_MAX="${JCODE_REMOTE_SERVER_ALIVE_COUNT_MAX:-1}"
SSH_OPTS=(
    -o BatchMode=yes
    -o ConnectTimeout="$SSH_CONNECT_TIMEOUT"
    -o ServerAliveInterval="$SSH_SERVER_ALIVE_INTERVAL"
    -o ServerAliveCountMax="$SSH_SERVER_ALIVE_COUNT_MAX"
)
RSYNC_SSH_COMMAND="${JCODE_REMOTE_RSYNC_SSH:-$SSH_BIN -o BatchMode=yes -o ConnectTimeout=$SSH_CONNECT_TIMEOUT -o ServerAliveInterval=$SSH_SERVER_ALIVE_INTERVAL -o ServerAliveCountMax=$SSH_SERVER_ALIVE_COUNT_MAX}"

remote_ssh() {
    "$SSH_BIN" "${SSH_OPTS[@]}" "$REMOTE" "$@"
}

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

profile_name=""
for ((i=0; i<${#POSITIONAL[@]}; i++)); do
    case "${POSITIONAL[$i]}" in
        --profile)
            if [[ $((i + 1)) -lt ${#POSITIONAL[@]} ]]; then
                profile_name="${POSITIONAL[$((i + 1))]}"
            fi
            ;;
        --profile=*)
            profile_name="${POSITIONAL[$i]#--profile=}"
            ;;
    esac
done

if [[ "$RELEASE" -eq 1 || "$profile_name" == "release" ]]; then
    build_mode="release"
elif [[ -n "$profile_name" && "$profile_name" != "dev" ]]; then
    build_mode="$profile_name"
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

local_git_hash=""
local_git_date=""
local_git_tag=""
local_git_dirty="0"
local_changelog_raw=""
if command -v git >/dev/null 2>&1 && git -C "$LOCAL_DIR" rev-parse --git-dir >/dev/null 2>&1; then
    local_git_hash="$(git -C "$LOCAL_DIR" rev-parse --short HEAD 2>/dev/null || true)"
    local_git_date="$(git -C "$LOCAL_DIR" log -1 --format=%ci 2>/dev/null || true)"
    local_git_tag="$(git -C "$LOCAL_DIR" describe --tags --always 2>/dev/null || true)"
    local_changelog_raw="$(git -C "$LOCAL_DIR" log -700 --format='%h|%ct|%D|%s' 2>/dev/null || true)"
    if [[ -n "$(git -C "$LOCAL_DIR" status --porcelain 2>/dev/null || true)" ]]; then
        local_git_dirty="1"
    fi
fi

echo "=== Remote Cargo on $REMOTE ==="
echo "Local:   $LOCAL_DIR"
echo "Remote:  $REMOTE_DIR"
echo "Command: ${CARGO_CMD[*]}"
echo "Mode:    $build_mode"
echo "SSH timeout: ${SSH_CONNECT_TIMEOUT}s"

echo ""
echo "[0/3] Checking remote SSH..."
if ! preflight_output="$(remote_ssh "printf 'jcode-remote-ok\\n'" 2>&1)"; then
    echo "error: remote host '$REMOTE' is not reachable within ${SSH_CONNECT_TIMEOUT}s" >&2
    echo "$preflight_output" >&2
    echo "hint: set JCODE_REMOTE_CARGO=0 to force local cargo, or fix JCODE_REMOTE_HOST/JCODE_REMOTE_CONNECT_TIMEOUT." >&2
    exit 75
fi

if [[ "$SYNC_SOURCE" -eq 1 ]]; then
    echo ""
    echo "[1/3] Syncing source files..."
    remote_ssh "$(printf 'mkdir -p %q' "$REMOTE_DIR")"
    "$RSYNC_BIN" -avz --delete \
        -e "$RSYNC_SSH_COMMAND" \
        --exclude 'target/' \
        --exclude '.git/' \
        --exclude '*.log' \
        --exclude '.claude/' \
        --exclude '.codex-socktest/' \
        --exclude '.jcode/' \
        --exclude '.tmp/' \
        --exclude '.wrangler/' \
        --exclude 'tmp/' \
        --exclude 'node_modules/' \
        --exclude 'assets/demos/' \
        --exclude 'assets/readme/' \
        "$LOCAL_DIR/" "$REMOTE:$REMOTE_DIR/"

    metadata_file="$(mktemp)"
    trap 'rm -f "$metadata_file"' EXIT
    {
        printf 'git_hash=%s\n' "$local_git_hash"
        printf 'git_date=%s\n' "$local_git_date"
        printf 'git_tag=%s\n' "$local_git_tag"
        printf 'git_dirty=%s\n' "$local_git_dirty"
        printf 'changelog_raw<<JCODE_CHANGELOG_EOF\n%s\nJCODE_CHANGELOG_EOF\n' "$local_changelog_raw"
    } > "$metadata_file"
    "$RSYNC_BIN" -avz -e "$RSYNC_SSH_COMMAND" "$metadata_file" "$REMOTE:$REMOTE_DIR/.jcode-build-meta"
else
    echo ""
    echo "[1/3] Skipping source sync (--no-sync)"
fi

printf -v REMOTE_CARGO_CMD '%q ' "${CARGO_CMD[@]}"
printf -v REMOTE_INNER_CMD 'cd %q && env JCODE_BUILD_METADATA_FILE=.jcode-build-meta %s' "$REMOTE_DIR" "$REMOTE_CARGO_CMD"
printf -v REMOTE_RUN_CMD 'sh -lc %q' "$REMOTE_INNER_CMD"
echo ""
echo "[2/3] Running on remote..."
remote_ssh "$REMOTE_RUN_CMD 2>&1"

echo ""
if [[ "$sync_back" -eq 1 ]]; then
    printf -v REMOTE_TEST_CMD 'test -f %q' "$REMOTE_DIR/$BINARY_PATH"
    if remote_ssh "$REMOTE_TEST_CMD"; then
        echo "[3/3] Syncing built artifact back..."
        mkdir -p "$(dirname "$LOCAL_DIR/$BINARY_PATH")"
        "$RSYNC_BIN" -avz -e "$RSYNC_SSH_COMMAND" "$REMOTE:$REMOTE_DIR/$BINARY_PATH" "$LOCAL_DIR/$BINARY_PATH"
        echo ""
        echo "=== Remote cargo complete ==="
        ls -la "$LOCAL_DIR/$BINARY_PATH"
    else
        echo "[3/3] Skipping sync-back: $BINARY_PATH not found on remote"
    fi
else
    echo "[3/3] Skipping binary sync-back"
fi
