#!/usr/bin/env bash
# Install the current release binary into the immutable version store,
# update the stable + current channel symlinks, and point the launcher at current.
#
# Paths after install:
# - ~/.jcode/builds/versions/<hash>/jcode (immutable)
# - ~/.jcode/builds/stable/jcode -> .../versions/<hash>/jcode
# - ~/.jcode/builds/current/jcode -> .../versions/<hash>/jcode
# - ~/.local/bin/jcode -> ~/.jcode/builds/current/jcode (launcher)
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
  JCODE_BUILD_GIT_HASH="$git_hash" \
    JCODE_BUILD_GIT_DATE="$git_date" \
    JCODE_BUILD_GIT_DIRTY="$git_dirty" \
    cargo build --profile "$profile" --manifest-path "$repo_root/Cargo.toml"
else
  cargo build --profile "$profile" --manifest-path "$repo_root/Cargo.toml"
fi
bin="$repo_root/target/$profile/jcode"

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

# Keep builds/manifest.json's "stable" field in sync with the marker and
# symlink. Nothing else writes this field (the selfdev promote flow that did
# was removed), so without this the manifest drifts stale and points at a
# pruned version.
#
# The update runs under a cross-process lock (manifest.json.lock): a plain
# lock file created with exclusive-create semantics. Today only the installers
# take this lock (master's runtime BuildManifest::save is an unlocked write),
# so this serializes concurrent installs; the holder format and prove-gone
# recovery rule are kept compatible with the cross-runtime file-lock protocol
# proposed for the jcode-storage side, so the runtime can adopt the same lock
# later without changing the installers. Recovery never guesses from age
# alone: holders record their PID in the lock, and a lock is broken only when
# its recorded PID is provably gone; PID-less locks fall back to the 60s age
# rule.
manifest="$builds_dir/manifest.json"
if [ -f "$manifest" ] && command -v jq >/dev/null 2>&1; then
  lock="${manifest}.lock"
  lock_acquired=0
  lock_deadline=$(( $(date +%s) + 65 ))
  while [ "$(date +%s)" -lt "$lock_deadline" ]; do
    if (set -C; printf 'pid=%s\n' "$$" > "$lock") 2>/dev/null; then
      lock_acquired=1
      break
    fi
    if [ -f "$lock" ]; then
      # Tolerant holder parse: accept "pid=<n> started=<marker>" / "pid=<n>
      # acquired_at=..." (cross-runtime protocol holders) as well as bare
      # "pid=<n>" (shell/legacy writers), so any future runtime holder is
      # recognized by PID and never falls through to the age rule. pid=0 is
      # rejected outright: kill(0) targets a whole process group, not a
      # process.
      holder_pid=$(sed -n 's/^pid=\(0*[1-9][0-9]*\).*/\1/p' "$lock" 2>/dev/null | head -1)
      if [ -n "$holder_pid" ]; then
        # Liveness probe mirroring the cross-runtime protocol: prove-gone
        # only. kill -0 failure is ambiguous (ESRCH = gone, EPERM = exists but
        # another user's), so confirm with ps (lists any user's process);
        # without ps presume the holder lives: a lock is never broken on
        # "cannot tell".
        holder_alive=0
        if kill -0 "$holder_pid" 2>/dev/null; then
          holder_alive=1
        elif ! command -v ps >/dev/null 2>&1; then
          holder_alive=1
        elif ps -p "$holder_pid" >/dev/null 2>&1; then
          holder_alive=1
        fi
        if [ "$holder_alive" != 1 ]; then
          rm -f "$lock"
          continue
        fi
      else
        # PID-less lock (legacy writer): fall back to the age rule.
        # GNU stat comes first: its -f is filesystem mode, and "stat -f %m"
        # fails (rc=1) while still printing filesystem status to stdout, which
        # a $(...) would capture as non-numeric garbage and then blow up the
        # age arithmetic below with a fatal error. BSD stat fails -c cleanly
        # (empty stdout), so trying -c before -f works on both platforms.
        lock_mtime=$(stat -c %Y "$lock" 2>/dev/null || stat -f %m "$lock" 2>/dev/null || echo 0)
        lock_age=$(( $(date +%s) - lock_mtime ))
        if [ "$lock_age" -ge 60 ]; then
          rm -f "$lock"
          continue
        fi
      fi
    fi
    # Fractional sleep when the platform supports it, else 1s granularity.
    sleep 0.025 2>/dev/null || sleep 1
  done
  if [ "$lock_acquired" = 1 ]; then
    if jq --arg s "$hash" '.stable = $s' "$manifest" > "$manifest.tmp" \
      && jq -e . "$manifest.tmp" >/dev/null 2>&1; then
      mv "$manifest.tmp" "$manifest"
    else
      rm -f "$manifest.tmp"
    fi
    rm -f "$lock"
  fi
fi

# Update current symlink + marker
current_dir="$builds_dir/current"
mkdir -p "$current_dir"
ln -sfn "$version_dir/jcode" "$current_dir/jcode"
printf '%s\n' "$hash" > "$builds_dir/current-version"

# Update launcher path to current channel
install_dir="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$install_dir"
ln -sfn "$current_dir/jcode" "$install_dir/jcode"

echo "Installed: $version_dir/jcode"
echo "Updated stable symlink: $stable_dir/jcode -> $version_dir/jcode"
echo "Updated current symlink: $current_dir/jcode -> $version_dir/jcode"
echo "Updated launcher symlink: $install_dir/jcode -> $current_dir/jcode"

# Configure supported desktop launch hotkeys as part of installation. This is
# idempotent and best-effort because headless installs may not expose a desktop
# session; the first interactive launch retries automatically.
case "$(uname -s)" in
  Darwin)
    if "$install_dir/jcode" setup-launcher </dev/null >/dev/null 2>&1; then
      echo "Installed macOS launcher and turn-notification broker."
    fi
    if "$install_dir/jcode" setup-hotkey </dev/null >/dev/null 2>&1; then
      echo "Configured system-wide jcode launch hotkeys (when supported)."
    fi
    ;;
  Linux)
    if "$install_dir/jcode" setup-hotkey </dev/null >/dev/null 2>&1; then
      echo "Configured system-wide jcode launch hotkeys (when supported)."
    fi
    ;;
esac

# Gracefully reload any running background server onto the binary we just
# installed (issue #291). `server reload` only reloads when the running daemon
# is genuinely older, hands live headless/swarm sessions to the new process, and
# is a no-op when no server is running, so it is safe to call unconditionally.
if [ "${JCODE_SKIP_SERVER_RELOAD:-}" != "1" ]; then
  if "$install_dir/jcode" server reload </dev/null >/dev/null 2>&1; then
    echo "Reloaded the running jcode server onto $hash (if one was active)."
  fi
fi

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$install_dir"; then
  echo ""
  echo "Tip: add $install_dir to PATH if needed."
fi

# Ensure the launcher dir is on PATH for bash, zsh and fish in future shells.
# shellcheck source=scripts/lib/configure_path.sh
. "$(dirname "$0")/lib/configure_path.sh"
jcode_configure_path "$install_dir"
