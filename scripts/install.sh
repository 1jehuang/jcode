#!/usr/bin/env bash
set -euo pipefail

REPO="1jehuang/jcode"
IS_WINDOWS=false
IS_TERMUX=false

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
err()  { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

OS="$(uname -s)"
ARCH="$(uname -m)"

if [ -n "${TERMUX_VERSION:-}" ] || [ "${PREFIX:-}" = "/data/data/com.termux/files/usr" ] || [ -d "/data/data/com.termux/files/usr" ]; then
  IS_TERMUX=true
fi

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  ARTIFACT="jcode-linux-x86_64" ;;
      aarch64|arm64) ARTIFACT="jcode-linux-aarch64" ;;
      *)       err "Unsupported Linux architecture: $ARCH" ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64)   ARTIFACT="jcode-macos-aarch64" ;;
      x86_64)  ARTIFACT="jcode-macos-x86_64" ;;
      *)       err "Unsupported macOS architecture: $ARCH" ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*)
    IS_WINDOWS=true
    case "$ARCH" in
      x86_64|AMD64)  ARTIFACT="jcode-windows-x86_64" ;;
      aarch64|arm64|ARM64) ARTIFACT="jcode-windows-aarch64" ;;
      *)       err "Unsupported Windows architecture: $ARCH" ;;
    esac
    ;;
  *)
    err "Unsupported OS: $OS (try building from source: https://github.com/$REPO)"
    ;;
esac

if [ "$IS_WINDOWS" = true ]; then
  INSTALL_DIR="${JCODE_INSTALL_DIR:-$LOCALAPPDATA/jcode/bin}"
else
  INSTALL_DIR="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
fi

# Extract the tag_name value, working for both pretty-printed (multi-line) and
# compact (single-line) GitHub API JSON. `grep -o` isolates just the tag_name
# field so `cut` no longer matches an unrelated string like the release url.
VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep -o '"tag_name" *: *"[^"]*"' | head -1 | cut -d'"' -f4)
[ -n "$VERSION" ] || err "Failed to determine latest version"

URL_TGZ="https://github.com/$REPO/releases/download/$VERSION/$ARTIFACT.tar.gz"
URL_BIN="https://github.com/$REPO/releases/download/$VERSION/$ARTIFACT"

if [ "$IS_WINDOWS" = true ]; then
  EXE=".exe"
  builds_dir="$LOCALAPPDATA/jcode/builds"
else
  EXE=""
  builds_dir="$HOME/.jcode/builds"
fi
stable_dir="$builds_dir/stable"
current_dir="$builds_dir/current"
version_dir="$builds_dir/versions"
launcher_path="$INSTALL_DIR/jcode${EXE}"

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

mkdir -p "$INSTALL_DIR" "$stable_dir" "$current_dir" "$version_dir"

version="${VERSION#v}"
dest_version_dir="$version_dir/$version"
mkdir -p "$dest_version_dir"

bin_name="jcode${EXE}"

if [ "$download_mode" = "tar" ]; then
  tar xzf "$tmpdir/jcode.download" -C "$tmpdir"
  src_bin="$tmpdir/${ARTIFACT}${EXE}"
  [ -f "$src_bin" ] || err "Downloaded archive did not contain expected binary: ${ARTIFACT}${EXE}"
  find "$tmpdir" -maxdepth 1 -type f \( -name "${ARTIFACT}${EXE}.bin" -o -name 'libssl.so*' -o -name 'libcrypto.so*' \) \
    -exec cp -f {} "$dest_version_dir/" \;
  mv "$src_bin" "$dest_version_dir/$bin_name"
elif [ "$download_mode" = "bin" ]; then
  mv "$tmpdir/jcode.download" "$dest_version_dir/$bin_name"
else
  info "No prebuilt asset found for $ARTIFACT in $VERSION; building from source..."
  command -v git >/dev/null 2>&1 || err "git is required to build from source"
  command -v cargo >/dev/null 2>&1 || err "cargo is required to build from source"

  src_dir="$tmpdir/jcode-src"
  git clone --depth 1 --branch "$VERSION" "https://github.com/$REPO.git" "$src_dir" \
    || err "Failed to clone $REPO at $VERSION"
  cargo build --release --manifest-path "$src_dir/Cargo.toml" \
    || err "cargo build failed while building $REPO from source"

  src_bin="$src_dir/target/release/$bin_name"
  [ -f "$src_bin" ] || err "Built binary not found at $src_bin"
  cp "$src_bin" "$dest_version_dir/$bin_name"
fi

chmod +x "$dest_version_dir/$bin_name" 2>/dev/null || true

if [ "$IS_TERMUX" = true ] && [ "$IS_WINDOWS" = false ]; then
  termux_glibc_dir="/data/data/com.termux/files/usr/glibc/lib"
  termux_glibc_linker=""
  case "$ARCH" in
    aarch64|arm64) termux_glibc_linker="$termux_glibc_dir/ld-linux-aarch64.so.1" ;;
    x86_64) termux_glibc_linker="$termux_glibc_dir/ld-linux-x86-64.so.2" ;;
  esac
  if [ "$OS" = "Linux" ] && [ -n "$termux_glibc_linker" ]; then
    if [ -x "$termux_glibc_linker" ]; then
      if command -v patchelf >/dev/null 2>&1; then
        patchelf --set-interpreter "$termux_glibc_linker" "$dest_version_dir/$bin_name" \
          || err "Failed to patch jcode ELF interpreter for Termux glibc"
        info "Patched Termux glibc ELF interpreter: $termux_glibc_linker"
      else
        info "Termux detected: install patchelf with 'pkg install patchelf' and rerun this installer if jcode fails to start."
      fi
    else
      info "Termux detected: install glibc with 'pkg install glibc' if jcode fails due to a missing dynamic linker."
    fi
  fi
fi

if [ "$IS_WINDOWS" = true ]; then
  cp -f "$dest_version_dir/$bin_name" "$stable_dir/$bin_name"
  printf '%s\n' "$version" > "$builds_dir/stable-version"
  cp -f "$stable_dir/$bin_name" "$launcher_path"
else
  ln -sfn "$dest_version_dir/$bin_name" "$stable_dir/$bin_name"
  printf '%s\n' "$version" > "$builds_dir/stable-version"
  if [ "$IS_TERMUX" = true ]; then
    rm -f "$launcher_path"
    cat > "$launcher_path" <<EOF
#!/usr/bin/env bash
unset LD_PRELOAD
exec "$stable_dir/$bin_name" "\$@"
EOF
    chmod +x "$launcher_path"
  else
    ln -sfn "$stable_dir/$bin_name" "$launcher_path"
  fi
fi

if [ "$(uname -s)" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$dest_version_dir/$bin_name" 2>/dev/null || true
fi

if [ "$(uname -s)" = "Darwin" ]; then
  if "$launcher_path" setup-hotkey </dev/null >/dev/null 2>&1; then
    mac_hotkey_ready=true
  else
    mac_hotkey_ready=false
  fi
fi

# Retire any background server still running the old binary so the freshly
# installed version is picked up without the user having to kill a daemon by
# hand (issue #291). We use the graceful `server reload` path, which hands live
# headless/swarm sessions to a newly-exec'd server instead of dropping them, and
# only reloads when the running server is genuinely older than what we just
# installed (so a newer/dev daemon is never downgraded). This is best-effort:
# it must never fail the install, and it is skipped when no server is running.
if [ "${JCODE_SKIP_SERVER_RELOAD:-}" != "1" ]; then
  reload_bin="$launcher_path"
  [ -x "$reload_bin" ] || reload_bin="$stable_dir/$bin_name"
  if [ -x "$reload_bin" ]; then
    if "$reload_bin" server reload </dev/null >/dev/null 2>&1; then
      info "Reloaded the running jcode server onto $VERSION (if one was active)."
    fi
  fi
fi

if [ "$IS_WINDOWS" = true ]; then
  win_install_dir=$(cygpath -w "$INSTALL_DIR" 2>/dev/null || echo "$INSTALL_DIR")
  echo ""
  info "✅ jcode $VERSION installed successfully!"
  echo ""
  if command -v jcode >/dev/null 2>&1; then
    info "Run 'jcode' to get started."
  else
    echo "  To start using jcode right now, run:"
    echo ""
    printf '    \033[1;32mexport PATH="%s:$PATH" && jcode\033[0m\n' "$INSTALL_DIR"
    echo ""
    echo "  To add jcode to PATH permanently (PowerShell):"
    echo ""
    printf '    \033[1;32m[Environment]::SetEnvironmentVariable("Path", "%s;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")\033[0m\n' "$win_install_dir"
  fi
else
  PATH_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
  added_to=""

  _have() { command -v "$1" >/dev/null 2>&1; }

  # Append the POSIX (bash/zsh/sh) PATH line to an rc file, idempotently.
  #   ensure_posix_rc <rc-file> <create:yes|no>
  # With create=yes the file (and parent dir) is created if missing; with
  # create=no we only touch files that already exist, so we never change how a
  # login shell resolves its startup files (e.g. creating ~/.bash_profile would
  # stop bash from reading ~/.profile).
  ensure_posix_rc() {
    rc="$1"; create="$2"
    if [ ! -f "$rc" ]; then
      [ "$create" = "yes" ] || return 0
      mkdir -p "$(dirname "$rc")"
    fi
    if ! grep -qF "$INSTALL_DIR" "$rc" 2>/dev/null; then
      printf '\n# Added by jcode installer\n%s\n' "$PATH_LINE" >> "$rc"
      added_to="$added_to $rc"
    fi
  }

  # fish uses its own syntax and does not read POSIX rc files.
  ensure_fish_rc() {
    create="$1"
    rc="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
    if [ ! -f "$rc" ]; then
      [ "$create" = "yes" ] || return 0
      mkdir -p "$(dirname "$rc")"
    fi
    if ! grep -qF "$INSTALL_DIR" "$rc" 2>/dev/null; then
      {
        printf '\n# Added by jcode installer\n'
        printf 'if not contains "%s" $PATH\n' "$INSTALL_DIR"
        printf '    set -gx PATH "%s" $PATH\n' "$INSTALL_DIR"
        printf 'end\n'
      } >> "$rc"
      added_to="$added_to $rc"
    fi
  }

  # zsh: ~/.zshenv is read for every zsh invocation (login, interactive and
  # scripts), so it is the most reliable single place to export PATH.
  if _have zsh || [ "$(uname -s)" = "Darwin" ] || [ -f "$HOME/.zshenv" ] || [ -f "$HOME/.zshrc" ]; then
    ensure_posix_rc "$HOME/.zshenv" yes
  fi

  # bash: ~/.bashrc for interactive shells, ~/.profile for login shells (macOS
  # Terminal, ssh, etc.). We only create ~/.profile, never ~/.bash_profile, so
  # we don't override an existing login-file lookup order.
  if _have bash || [ -f "$HOME/.bashrc" ] || [ -f "$HOME/.bash_profile" ]; then
    ensure_posix_rc "$HOME/.bashrc" yes
  fi
  ensure_posix_rc "$HOME/.profile" yes

  # fish: only set it up when fish is installed or already configured.
  if _have fish || [ -f "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ]; then
    ensure_fish_rc yes
  fi

  # Also patch other common startup files when they already exist, so we cover
  # users with custom login-shell setups without creating new files.
  for rc in "$HOME/.zshrc" "$HOME/.zprofile" "$HOME/.bash_profile"; do
    ensure_posix_rc "$rc" no
  done

  if [ -n "$added_to" ]; then
    info "Added $INSTALL_DIR to PATH in:$added_to"
  fi

  echo ""
  info "✅ jcode $VERSION installed successfully!"
  echo ""

  if [ "$(uname -s)" = "Darwin" ]; then
    if [ "${mac_hotkey_ready:-false}" = true ]; then
      info "Global hotkey ready: Cmd+; launches a new jcode from anywhere, system-wide"
    else
      info "Tip: run 'jcode setup-hotkey' so Cmd+; launches jcode system-wide on macOS"
    fi
  fi

  if command -v jcode >/dev/null 2>&1; then
    info "Run 'jcode' to get started."
  else
    echo "  To start using jcode right now, run:"
    echo ""
    printf '    \033[1;32mexport PATH="%s:\$PATH" && jcode\033[0m\n' "$INSTALL_DIR"
    echo ""
    echo "  Future terminal sessions will have jcode on PATH automatically."
  fi
fi
