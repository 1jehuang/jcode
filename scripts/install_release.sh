#!/usr/bin/env bash
# Install the current release binary into the immutable version store,
# update the stable channel symlink, and point the launcher at stable.
#
# Paths after install:
# - ~/.jcode/builds/versions/<hash>/jcode (immutable)
# - ~/.jcode/builds/stable/jcode -> .../versions/<hash>/jcode
# - ~/.local/bin/jcode -> ~/.jcode/builds/stable/jcode (launcher)
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

profile="${JCODE_RELEASE_PROFILE:-release-lto}"
if [[ "${1:-}" == "--fast" ]]; then
  profile="release"
  shift
fi

if [[ "$#" -gt 0 ]]; then
  echo "Usage: $0 [--fast]" >&2
  exit 1
fi

case "$profile" in
  release-lto)
    echo "Building with LTO (this takes a few minutes)..."
    ;;
  release)
    echo "Building fast release profile (no LTO)..."
    ;;
  *)
    echo "Unsupported profile: $profile (expected: release or release-lto)" >&2
    exit 1
    ;;
esac

cargo build --profile "$profile" --manifest-path "$repo_root/Cargo.toml"
bin="$repo_root/target/$profile/jcode"

if [[ ! -x "$bin" ]]; then
  echo "Release binary not found: $bin" >&2
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

# Update stable-version marker
printf '%s\n' "$hash" > "$builds_dir/stable-version"

# Update launcher path to stable channel
install_dir="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$install_dir"
ln -sfn "$stable_dir/jcode" "$install_dir/jcode"

echo "Installed: $version_dir/jcode"
echo "Updated stable symlink: $stable_dir/jcode -> $version_dir/jcode"
echo "Updated launcher symlink: $install_dir/jcode -> $stable_dir/jcode"

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$install_dir"; then
  echo ""
  echo "Tip: add $install_dir to PATH if needed."
fi
