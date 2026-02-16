#!/usr/bin/env bash
# Install the current release binary as the stable/release version.
#
# This updates ~/.jcode/builds/stable/jcode to point to a versioned copy
# of the current target/release/jcode binary.
#
# On Linux: ~/.local/bin/jcode should be a symlink to target/release/jcode
# On macOS: /usr/local/bin/jcode or ~/.local/bin/jcode
#
# This script does NOT touch the PATH symlink; it only manages stable builds.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
bin="$repo_root/target/release/jcode"

if [[ ! -x "$bin" ]]; then
  echo "Release binary not found or not executable: $bin" >&2
  echo "Run: cargo build --release" >&2
  exit 1
fi

hash=""
if command -v git >/dev/null 2>&1; then
  if git -C "$repo_root" rev-parse --git-dir >/dev/null 2>&1; then
    hash="$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || true)"
    if [[ -n "${hash}" ]] && [[ -n "$(git -C "$repo_root" status --porcelain 2>/dev/null || true)" ]]; then
      hash="${hash}-dirty"
    fi
  fi
fi

if [[ -z "$hash" ]]; then
  hash="$(date +%Y%m%d%H%M%S)"
fi

# Install versioned binary into ~/.jcode/builds/versions/<hash>/
builds_dir="$HOME/.jcode/builds"
version_dir="$builds_dir/versions/$hash"
mkdir -p "$version_dir"
install -m 755 "$bin" "$version_dir/jcode"

# Update stable symlink
stable_dir="$builds_dir/stable"
mkdir -p "$stable_dir"
ln -sfn "$version_dir/jcode" "$stable_dir/jcode"

echo "Installed: $version_dir/jcode"
echo "Updated stable symlink: $stable_dir/jcode -> $version_dir/jcode"

# Offer to create PATH symlink if not present
if [[ "$(uname)" == "Darwin" ]]; then
  install_dir="/usr/local/bin"
  if [[ ! -w "$install_dir" ]]; then
    install_dir="$HOME/.local/bin"
  fi
else
  install_dir="$HOME/.local/bin"
fi

if [[ ! -e "$install_dir/jcode" ]]; then
  echo ""
  echo "Tip: To add jcode to your PATH, run:"
  echo "  mkdir -p $install_dir"
  echo "  ln -sf $repo_root/target/release/jcode $install_dir/jcode"
  if [[ "$(uname)" == "Darwin" ]] && [[ "$install_dir" == "$HOME/.local/bin" ]]; then
    echo ""
    echo "  # Add to your shell profile if not already:"
    echo '  echo '\''export PATH="$HOME/.local/bin:$PATH"'\'' >> ~/.zshrc'
  fi
fi
