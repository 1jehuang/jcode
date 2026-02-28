#!/usr/bin/env bash
set -euo pipefail

# Quick release script - builds Linux + macOS locally and uploads to GitHub.
# macOS is cross-compiled via osxcross (~/.osxcross). Windows is built by CI.
#
# Setup (one-time):
#   1. Install osxcross at ~/.osxcross
#   2. rustup target add aarch64-apple-darwin
#   3. Add to ~/.cargo/config.toml:
#        [target.aarch64-apple-darwin]
#        linker = "aarch64-apple-darwin23.5-clang"
#
# Usage:
#   scripts/quick-release.sh v0.5.5              # tag + build + release
#   scripts/quick-release.sh v0.5.5 "Fix bug"    # with custom title
#   scripts/quick-release.sh --dry-run v0.5.5    # build only, don't publish

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
    shift
fi

VERSION="${1:?Usage: scripts/quick-release.sh [--dry-run] <version> [title]}"
TITLE="${2:-$VERSION}"

if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in format v0.5.4"
    exit 1
fi

cd "$(git rev-parse --show-toplevel)"

for cmd in gh cargo; do
    command -v "$cmd" &>/dev/null || { echo "Error: $cmd not found."; exit 1; }
done

[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
export PATH="$HOME/.osxcross/bin:$PATH"

# Verify osxcross is available
if ! command -v aarch64-apple-darwin23.5-clang &>/dev/null; then
    echo "Error: osxcross not found. Install at ~/.osxcross"
    exit 1
fi

if [[ -n "$(git status --porcelain -- src/ Cargo.toml Cargo.lock)" ]]; then
    echo "Warning: uncommitted changes in src/ or Cargo files."
    read -rp "Continue anyway? [y/N] " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || exit 1
fi

echo "=== Quick Release: $VERSION ==="
echo ""

DIST="$(mktemp -d)"
trap 'rm -rf "$DIST"' EXIT

OVERALL_START=$(date +%s)

# Build Linux + macOS in parallel
echo "▸ Building Linux x86_64 + macOS aarch64 in parallel..."

(
    JCODE_RELEASE_BUILD=1 cargo build --release 2>/dev/null
    cp target/release/jcode "$DIST/jcode-linux-x86_64"
    chmod +x "$DIST/jcode-linux-x86_64"
    (cd "$DIST" && tar czf jcode-linux-x86_64.tar.gz jcode-linux-x86_64)
    echo "  ✅ Linux done ($(( $(date +%s) - OVERALL_START ))s)"
) &
LINUX_PID=$!

(
    JCODE_RELEASE_BUILD=1 cargo build --release --target aarch64-apple-darwin 2>/dev/null
    cp target/aarch64-apple-darwin/release/jcode "$DIST/jcode-macos-aarch64"
    chmod +x "$DIST/jcode-macos-aarch64"
    (cd "$DIST" && tar czf jcode-macos-aarch64.tar.gz jcode-macos-aarch64)
    echo "  ✅ macOS done ($(( $(date +%s) - OVERALL_START ))s)"
) &
MACOS_PID=$!

wait $LINUX_PID || { echo "Error: Linux build failed"; exit 1; }
wait $MACOS_PID || { echo "Error: macOS build failed"; exit 1; }

BUILD_TIME=$(( $(date +%s) - OVERALL_START ))
echo ""
echo "Build time: ${BUILD_TIME}s"
ls -lh "$DIST"/*.tar.gz

# Verify binaries
file "$DIST/jcode-linux-x86_64" | grep -q 'ELF 64-bit' || { echo "Error: bad Linux binary"; exit 1; }
file "$DIST/jcode-macos-aarch64" | grep -q 'Mach-O 64-bit' || { echo "Error: bad macOS binary"; exit 1; }

if $DRY_RUN; then
    echo ""
    echo "Dry run complete. Binaries in: $DIST"
    trap - EXIT
    exit 0
fi

echo ""
echo "▸ Tagging $VERSION..."
if git tag -l "$VERSION" | grep -q "$VERSION"; then
    echo "  Tag already exists"
else
    git tag "$VERSION" -m "$TITLE"
    git push origin "$VERSION"
    echo "  Tag pushed (CI will add Windows)"
fi

echo "▸ Creating GitHub release..."
gh release create "$VERSION" \
    "$DIST/jcode-linux-x86_64.tar.gz" \
    "$DIST/jcode-macos-aarch64.tar.gz" \
    --title "$TITLE" \
    --generate-notes

TOTAL_TIME=$(( $(date +%s) - OVERALL_START ))
echo ""
echo "=== Released $VERSION in ${TOTAL_TIME}s ==="
echo "  ✅ Linux + macOS: available now"
echo "  ⏳ Windows: CI (~15 min)"
echo ""
echo "Users can now: jcode update"
