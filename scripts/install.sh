#!/usr/bin/env bash
set -euo pipefail

REPO="1jehuang/jcode"
INSTALL_DIR="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
err()  { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  ARTIFACT="jcode-linux-x86_64" ;;
      *)       err "Unsupported Linux architecture: $ARCH (only x86_64 supported)" ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64)   ARTIFACT="jcode-macos-aarch64" ;;
      x86_64)  ARTIFACT="jcode-macos-aarch64" ;; # Rosetta 2
      *)       err "Unsupported macOS architecture: $ARCH" ;;
    esac
    ;;
  *)
    err "Unsupported OS: $OS (try building from source: https://github.com/$REPO)"
    ;;
esac

VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)
[ -n "$VERSION" ] || err "Failed to determine latest version"

URL="https://github.com/$REPO/releases/download/$VERSION/$ARTIFACT.tar.gz"

info "Installing jcode $VERSION ($ARTIFACT)"
info "  from: $URL"
info "  to:   $INSTALL_DIR/jcode"

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

curl -fsSL "$URL" -o "$tmpdir/jcode.tar.gz"
tar xzf "$tmpdir/jcode.tar.gz" -C "$tmpdir"

mkdir -p "$INSTALL_DIR"
mv "$tmpdir/$ARTIFACT" "$INSTALL_DIR/jcode"
chmod +x "$INSTALL_DIR/jcode"

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
  printf '\n\033[1;33m%s\033[0m\n' "⚠  $INSTALL_DIR is not in your PATH. Add it:"
  echo ""
  echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
  echo ""
  echo "  Add that line to your ~/.bashrc, ~/.zshrc, or equivalent."
fi

info "✅ jcode $VERSION installed successfully!"
info ""
info "Run 'jcode' to get started."
