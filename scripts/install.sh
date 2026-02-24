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

URL_TGZ="https://github.com/$REPO/releases/download/$VERSION/$ARTIFACT.tar.gz"
URL_BIN="https://github.com/$REPO/releases/download/$VERSION/$ARTIFACT"

builds_dir="$HOME/.jcode/builds"
stable_dir="$builds_dir/stable"
version_dir="$builds_dir/versions"
launcher_path="$INSTALL_DIR/jcode"

EXISTING=""
if [ -x "$launcher_path" ]; then
  EXISTING=$("$launcher_path" --version 2>/dev/null | head -1 || echo "unknown")
fi

if [ -n "$EXISTING" ]; then
  if echo "$EXISTING" | grep -qF "${VERSION#v}"; then
    info "jcode $VERSION is already installed — reinstalling"
  else
    info "Updating jcode $EXISTING → $VERSION"
  fi
else
  info "Installing jcode $VERSION"
fi
info "  launcher: $launcher_path"

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

download_mode=""
if curl -fsSL "$URL_TGZ" -o "$tmpdir/jcode.download" 2>/dev/null; then
  download_mode="tar"
elif curl -fsSL "$URL_BIN" -o "$tmpdir/jcode.download" 2>/dev/null; then
  download_mode="bin"
fi

mkdir -p "$INSTALL_DIR" "$stable_dir" "$version_dir"

version="${VERSION#v}"
dest_version_dir="$version_dir/$version"
mkdir -p "$dest_version_dir"

if [ "$download_mode" = "tar" ]; then
  tar xzf "$tmpdir/jcode.download" -C "$tmpdir"
  src_bin="$tmpdir/$ARTIFACT"
  [ -f "$src_bin" ] || err "Downloaded archive did not contain expected binary: $ARTIFACT"
  mv "$src_bin" "$dest_version_dir/jcode"
elif [ "$download_mode" = "bin" ]; then
  mv "$tmpdir/jcode.download" "$dest_version_dir/jcode"
else
  info "No prebuilt asset found for $ARTIFACT in $VERSION; building from source..."
  command -v git >/dev/null 2>&1 || err "git is required to build from source"
  command -v cargo >/dev/null 2>&1 || err "cargo is required to build from source"

  src_dir="$tmpdir/jcode-src"
  git clone --depth 1 --branch "$VERSION" "https://github.com/$REPO.git" "$src_dir" \
    || err "Failed to clone $REPO at $VERSION"
  cargo build --release --manifest-path "$src_dir/Cargo.toml" \
    || err "cargo build failed while building $REPO from source"

  src_bin="$src_dir/target/release/jcode"
  [ -f "$src_bin" ] || err "Built binary not found at $src_bin"
  cp "$src_bin" "$dest_version_dir/jcode"
fi

chmod +x "$dest_version_dir/jcode"

ln -sfn "$dest_version_dir/jcode" "$stable_dir/jcode"
printf '%s\n' "$version" > "$builds_dir/stable-version"
ln -sfn "$stable_dir/jcode" "$launcher_path"

if [ "$(uname -s)" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$dest_version_dir/jcode" 2>/dev/null || true
fi

PATH_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""

if [ "$(uname -s)" = "Darwin" ]; then
  DEFAULT_RC="$HOME/.zshrc"
else
  DEFAULT_RC="$HOME/.bashrc"
fi

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
  added_to=""

  # Always ensure the default rc file has the PATH
  if [ ! -f "$DEFAULT_RC" ] || ! grep -qF "$INSTALL_DIR" "$DEFAULT_RC" 2>/dev/null; then
    printf '\n# Added by jcode installer\n%s\n' "$PATH_LINE" >> "$DEFAULT_RC"
    added_to="$added_to $DEFAULT_RC"
  fi

  # Also add to other existing rc files
  for rc in "$HOME/.zprofile" "$HOME/.bash_profile" "$HOME/.profile"; do
    if [ -f "$rc" ] && ! grep -qF "$INSTALL_DIR" "$rc" 2>/dev/null; then
      printf '\n# Added by jcode installer\n%s\n' "$PATH_LINE" >> "$rc"
      added_to="$added_to $rc"
    fi
  done

  info "Added $INSTALL_DIR to PATH in:$added_to"
fi

echo ""
info "✅ jcode $VERSION installed successfully!"
echo ""

if command -v jcode >/dev/null 2>&1; then
  info "Run 'jcode' to get started."
else
  echo "  To start using jcode, open a new terminal window, or run:"
  echo ""
  printf '    \033[1;32msource %s\033[0m\n' "$DEFAULT_RC"
  echo ""
  echo "  Then run:"
  echo ""
  printf '    \033[1;32mjcode\033[0m\n'
fi
