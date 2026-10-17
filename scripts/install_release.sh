#!/usr/bin/env bash
# Install the current release binary into the immutable version store,
# update the stable + current channel symlinks, and point the launcher at current.
#
# Paths after install:
# - ~/.kcode/builds/versions/<hash>/kcode (immutable)
# - ~/.kcode/builds/stable/kcode -> .../versions/<hash>/kcode
# - ~/.kcode/builds/current/kcode -> .../versions/<hash>/kcode
# - ~/.local/bin/kcode -> ~/.kcode/builds/current/kcode (launcher)
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
source "$repo_root/scripts/product_paths.sh"
product_root="${KCODE_HOME:-$HOME/.kcode}"
builds_dir="$product_root/builds"
install_dir="${KCODE_INSTALL_DIR:-$HOME/.local/bin}"
require_kcode_owned_path "$product_root"
require_kcode_owned_path "$builds_dir"
require_kcode_owned_path "$install_dir"

profile="${KCODE_RELEASE_PROFILE:-release-lto}"
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

git_hash=""
git_date=""
git_dirty="0"
if command -v git >/dev/null 2>&1; then
  if git -C "$repo_root" rev-parse --git-dir >/dev/null 2>&1; then
    git_hash="$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || true)"
    git_date="$(git -C "$repo_root" log -1 --format=%ci 2>/dev/null || true)"
    if [[ -n "${git_hash}" ]] && [[ -n "$(git -C "$repo_root" status --porcelain 2>/dev/null || true)" ]]; then
      git_dirty="1"
    fi
  fi
fi

hash="$git_hash"
if [[ -n "$hash" ]] && [[ "$git_dirty" == "1" ]]; then
  hash="${hash}-dirty"
fi
if [[ -z "$hash" ]]; then
  hash="$(date +%Y%m%d%H%M%S)"
fi

if [[ -n "$git_hash" ]]; then
  KCODE_BUILD_GIT_HASH="$git_hash" \
    KCODE_BUILD_GIT_DATE="$git_date" \
    KCODE_BUILD_GIT_DIRTY="$git_dirty" \
    cargo build --profile "$profile" --manifest-path "$repo_root/Cargo.toml"
else
  cargo build --profile "$profile" --manifest-path "$repo_root/Cargo.toml"
fi
bin="$repo_root/target/$profile/kcode"

if [[ ! -x "$bin" ]]; then
  echo "Release binary not found: $bin" >&2
  exit 1
fi

if [[ -n "$git_hash" ]]; then
  expected_git_identity="($git_hash)"
  if [[ "$git_dirty" == "1" ]]; then
    expected_git_identity="($git_hash, dirty)"
  fi
  if [[ "$($bin --version)" != *"$expected_git_identity"* ]]; then
    echo "Release binary does not report expected git identity: $expected_git_identity" >&2
    exit 1
  fi
fi

# Install versioned binary into ~/.kcode/builds/versions/<hash>/
version_dir="$builds_dir/versions/$hash"
require_kcode_owned_path "$version_dir/kcode"
mkdir -p "$version_dir"
install -m 755 "$bin" "$version_dir/kcode"

# Update stable symlink
stable_dir="$builds_dir/stable"
require_kcode_owned_path "$stable_dir"
require_kcode_owned_path "$builds_dir/stable-version"
mkdir -p "$stable_dir"
ln -sfn "$version_dir/kcode" "$stable_dir/kcode"

# Update stable-version marker
printf '%s\n' "$hash" > "$builds_dir/stable-version"

# Update current symlink + marker
current_dir="$builds_dir/current"
require_kcode_owned_path "$current_dir"
require_kcode_owned_path "$builds_dir/current-version"
mkdir -p "$current_dir"
ln -sfn "$version_dir/kcode" "$current_dir/kcode"
printf '%s\n' "$hash" > "$builds_dir/current-version"

# Update launcher path to current channel
require_kcode_owned_path "$install_dir"
mkdir -p "$install_dir"
ln -sfn "$current_dir/kcode" "$install_dir/kcode"

echo "Installed: $version_dir/kcode"
echo "Updated stable symlink: $stable_dir/kcode -> $version_dir/kcode"
echo "Updated current symlink: $current_dir/kcode -> $version_dir/kcode"
echo "Updated launcher symlink: $install_dir/kcode -> $current_dir/kcode"

# Configure supported desktop launch hotkeys as part of installation. This is
# idempotent and best-effort because headless installs may not expose a desktop
# session; the first interactive launch retries automatically.
case "$(uname -s)" in
  Darwin)
    if "$install_dir/kcode" setup-launcher </dev/null >/dev/null 2>&1; then
      echo "Installed macOS launcher and turn-notification broker."
    fi
    if "$install_dir/kcode" setup-hotkey </dev/null >/dev/null 2>&1; then
      echo "Configured system-wide kcode launch hotkeys (when supported)."
    fi
    ;;
  Linux)
    if "$install_dir/kcode" setup-hotkey </dev/null >/dev/null 2>&1; then
      echo "Configured system-wide kcode launch hotkeys (when supported)."
    fi
    ;;
esac

# Gracefully reload any running background server onto the binary we just
# installed (issue #291). `server reload` only reloads when the running daemon
# is genuinely older, hands live headless/swarm sessions to the new process, and
# is a no-op when no server is running, so it is safe to call unconditionally.
if [ "${KCODE_SKIP_SERVER_RELOAD:-}" != "1" ]; then
  if "$install_dir/kcode" server reload </dev/null >/dev/null 2>&1; then
    echo "Reloaded the running kcode server onto $hash (if one was active)."
  fi
fi

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$install_dir"; then
  echo ""
  echo "Tip: add $install_dir to PATH if needed."
fi

# Ensure the launcher dir is on PATH for bash, zsh and fish in future shells.
# shellcheck source=scripts/lib/configure_path.sh
. "$(dirname "$0")/lib/configure_path.sh"
kcode_configure_path "$install_dir"
