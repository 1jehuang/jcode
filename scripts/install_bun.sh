#!/usr/bin/env bash
# Bun-native installer for jcode.
# Install via: bunx jcode-install
#          or: bun run --bun https://jcode.sh/install_bun.sh
# Falls back to curl/tar when Bun is not available.
# Requires bash >= 3.2.  Do not add bashisms beyond what install.sh already uses.
set -euo pipefail

REPO="1jehuang/jcode"
RELEASE_METADATA_BASE="${JCODE_RELEASE_METADATA_BASE:-https://jcode.sh/releases}"
INSTALL_STAGE="startup"
INSTALL_SUCCEEDED=0
INSTALL_OS="unknown"
INSTALL_ARCH="unknown"
INSTALL_VERSION="unknown"
FORCE_BUN=0
INSTALL_METHOD="shell"
tmpdir=""

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
err()  { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Telemetry (mirrors install.sh)
# ---------------------------------------------------------------------------
valid_release_tag() {
  printf '%s' "$1" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+([+.-][[:alnum:].-]+)?$'
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print tolower($1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print tolower($1)}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file" | awk '{print tolower($NF)}'
  elif command -v bun >/dev/null 2>&1; then
    # Bun 1.2+ has crypto.hash
    bun -e "const f=Bun.file('$file'); const h=await Bun.CryptoHasher.hash('sha256',f); process.stdout.write(h.hex())"
  else
    return 1
  fi
}

valid_conversion_id() {
  printf '%s' "${JCODE_INSTALL_CONVERSION_ID:-}" |
    grep -Eiq '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
}

telemetry_value() {
  printf '%s' "$1" | tr -cd '[:alnum:]_. -' | cut -c1-100
}

report_install_funnel() {
  local stage="$1" outcome="$2" failure_stage="${3:-}"
  [ "${JCODE_NO_TELEMETRY:-}" != "1" ] || return 0
  [ "${DO_NOT_TRACK:-}" != "1" ] || return 0
  valid_conversion_id || return 0
  local payload
  payload=$(printf '{"id":"%s","event":"install_funnel","version":"%s","os":"%s","arch":"%s","conversion_id":"%s","stage":"%s","outcome":"%s","source":"installer","install_method":"%s","failure_stage":"%s"}' \
    "$JCODE_INSTALL_CONVERSION_ID" \
    "$(telemetry_value "$INSTALL_VERSION")" \
    "$(telemetry_value "$INSTALL_OS")" \
    "$(telemetry_value "$INSTALL_ARCH")" \
    "$JCODE_INSTALL_CONVERSION_ID" \
    "$(telemetry_value "$stage")" \
    "$(telemetry_value "$outcome")" \
    "$(telemetry_value "$INSTALL_METHOD")" \
    "$(telemetry_value "$failure_stage")")
  curl -fsS --max-time 2 -H 'Content-Type: application/json' \
    --data "$payload" https://telemetry.jcode.sh/v1/event >/dev/null 2>&1 || true
}

persist_install_conversion_id() {
  [ "${JCODE_NO_TELEMETRY:-}" != "1" ] || return 0
  [ "${DO_NOT_TRACK:-}" != "1" ] || return 0
  valid_conversion_id || return 0
  local jcode_home="${JCODE_HOME:-$HOME/.jcode}"
  mkdir -p "$jcode_home" 2>/dev/null || return 0
  (umask 077; printf '%s\n' "$JCODE_INSTALL_CONVERSION_ID" > "$jcode_home/install_conversion_id") \
    2>/dev/null || return 0
  chmod 600 "$jcode_home/install_conversion_id" 2>/dev/null || true
}

install_exit() {
  local status=$?
  trap - EXIT
  set +e
  [ -z "$tmpdir" ] || rm -rf "$tmpdir"
  if [ "$INSTALL_SUCCEEDED" = "1" ] && [ "$status" = "0" ]; then
    report_install_funnel "installer_finish" "success" ""
  else
    report_install_funnel "installer_finish" "failure" "$INSTALL_STAGE"
  fi
  exit "$status"
}
trap install_exit EXIT

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
for arg in "$@"; do
  case "$arg" in
    --bun) FORCE_BUN=1 ;;
  esac
done

# ---------------------------------------------------------------------------
# Detect Bun availability and select download method
# ---------------------------------------------------------------------------
detect_bun() {
  if command -v bun >/dev/null 2>&1; then
    INSTALL_METHOD="bun"
    return 0
  fi
  if [ "$FORCE_BUN" = "1" ]; then
    err "--bun specified but Bun is not installed. Install Bun first: https://bun.sh"
  fi
  INSTALL_METHOD="shell"
  return 1
}

has_bun=0
if detect_bun; then
  has_bun=1
fi

# ---------------------------------------------------------------------------
# Fetch helpers (Bun-native or curl)
# ---------------------------------------------------------------------------
bun_fetch() {
  local url="$1"
  if [ "$has_bun" = "1" ]; then
    bun -e "const r=await fetch('$url'); if(!r.ok) throw new Error('HTTP '+r.status); process.stdout.write(await r.text())"
  else
    curl -fsSL --retry 2 --connect-timeout 10 "$url"
  fi
}

bun_download_file() {
  local url="$1" dest="$2"
  if [ "$has_bun" = "1" ]; then
    bun -e "
      const r=await fetch('$url'); if(!r.ok) throw new Error('HTTP '+r.status);
      await Bun.write('$dest', r);
    "
  else
    curl -fsSL --retry 2 --connect-timeout 10 "$url" -o "$dest"
  fi
}

# ---------------------------------------------------------------------------
# Platform detection (mirrors install.sh exactly)
# ---------------------------------------------------------------------------
INSTALL_STAGE="platform_detection"
IS_WINDOWS=false
IS_TERMUX=false
OS="$(uname -s)"
ARCH="$(uname -m)"
INSTALL_OS="$OS"
INSTALL_ARCH="$ARCH"

if [ -n "${TERMUX_VERSION:-}" ] || [ "${PREFIX:-}" = "/data/data/com.termux/files/usr" ] || [ -d "/data/data/com.termux/files/usr" ]; then
  IS_TERMUX=true
fi

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)       ARTIFACT="jcode-linux-x86_64" ;;
      aarch64|arm64) ARTIFACT="jcode-linux-aarch64" ;;
      *)            err "Unsupported Linux architecture: $ARCH" ;;
    esac ;;
  Darwin)
    case "$ARCH" in
      arm64)   ARTIFACT="jcode-macos-aarch64" ;;
      x86_64)  ARTIFACT="jcode-macos-x86_64" ;;
      *)       err "Unsupported macOS architecture: $ARCH" ;;
    esac ;;
  MINGW*|MSYS*|CYGWIN*)
    IS_WINDOWS=true
    WINDOWS_ARCH=""
    for candidate in "${PROCESSOR_ARCHITEW6432:-}" "${PROCESSOR_ARCHITECTURE:-}" "$ARCH"; do
      case "$candidate" in aarch64|AARCH64|arm64|Arm64|ARM64) WINDOWS_ARCH="aarch64"; break ;; esac
    done
    if [ -z "$WINDOWS_ARCH" ]; then
      for candidate in "${PROCESSOR_ARCHITEW6432:-}" "${PROCESSOR_ARCHITECTURE:-}" "$ARCH"; do
        case "$candidate" in x86_64|X64|AMD64) WINDOWS_ARCH="x86_64"; break ;; esac
      done
    fi
    case "$WINDOWS_ARCH" in
      x86_64)  ARTIFACT="jcode-windows-x86_64" ;;
      aarch64) ARTIFACT="jcode-windows-aarch64" ;;
      *)       err "Unsupported Windows architecture: $ARCH" ;;
    esac ;;
  *)
    err "Unsupported OS: $OS (try building from source: https://github.com/$REPO)" ;;
esac

report_install_funnel "installer_start" "success" ""

# ---------------------------------------------------------------------------
# Install directory
# ---------------------------------------------------------------------------
if [ "$IS_WINDOWS" = true ]; then
  INSTALL_DIR="${JCODE_INSTALL_DIR:-$LOCALAPPDATA/jcode/bin}"
else
  INSTALL_DIR="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
fi

# ---------------------------------------------------------------------------
# Version resolution (same dual-source strategy as install.sh)
# ---------------------------------------------------------------------------
INSTALL_STAGE="release_lookup"
VERSION="${JCODE_VERSION:-}"
if [ -z "$VERSION" ]; then
  METADATA_VERSION=$(bun_fetch "$RELEASE_METADATA_BASE/latest/version" 2>/dev/null | tr -d '\r\n' || true)
  LATEST_RELEASE_URL=$(curl -fsSIL --retry 2 --connect-timeout 10 \
    -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null || true)
  case "$LATEST_RELEASE_URL" in */releases/tag/*) GITHUB_VERSION="${LATEST_RELEASE_URL##*/}" ;; *) GITHUB_VERSION="" ;; esac
  if valid_release_tag "$GITHUB_VERSION"; then
    VERSION="$GITHUB_VERSION"
  elif valid_release_tag "$METADATA_VERSION"; then
    VERSION="$METADATA_VERSION"
    info "GitHub release lookup unavailable; using cached jcode.sh metadata ($VERSION)."
  fi
fi
valid_release_tag "$VERSION" || err "Failed to determine latest version"
INSTALL_VERSION="${VERSION#v}"

GITHUB_RELEASE_BASE="https://github.com/$REPO/releases/download/$VERSION"

if [ "$IS_WINDOWS" = true ]; then
  EXE=".exe"; builds_dir="$LOCALAPPDATA/jcode/builds"
else
  EXE=""; builds_dir="$HOME/.jcode/builds"
fi
stable_dir="$builds_dir/stable"
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
    info "Updating jcode $EXISTING -> $VERSION"
  fi
else
  info "Installing jcode $VERSION"
fi
info "  launcher: $launcher_path"
info "  method:   $INSTALL_METHOD"

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------
tmpdir=$(mktemp -d)

INSTALL_STAGE="artifact_download"
download_mode=""
downloaded_asset=""
DOWNLOAD_BASES=$(bun_fetch "$RELEASE_METADATA_BASE/$VERSION/download-bases" 2>/dev/null || true)
DOWNLOAD_BASES=$(printf '%s\n%s\n' "$DOWNLOAD_BASES" "$GITHUB_RELEASE_BASE" |
  awk '/^https:\/\/[^[:space:]]+$/ && !seen[$0]++')

for candidate in "$ARTIFACT.tar.gz" "$ARTIFACT$EXE"; do
  while IFS= read -r base; do
    [ -n "$base" ] || continue
    if bun_download_file "${base%/}/$candidate" "$tmpdir/jcode.download" 2>/dev/null; then
      downloaded_asset="$candidate"
      case "$candidate" in *.tar.gz) download_mode="tar" ;; *) download_mode="bin" ;; esac
      break 2
    fi
  done <<EOF
$DOWNLOAD_BASES
EOF
done

# ---------------------------------------------------------------------------
# SHA-256 verification
# ---------------------------------------------------------------------------
if [ -n "$download_mode" ]; then
  INSTALL_STAGE="artifact_verification"
  EXPECTED_SHA256=""
  for checksum_url in "$RELEASE_METADATA_BASE/$VERSION/SHA256SUMS" "$GITHUB_RELEASE_BASE/SHA256SUMS"; do
    CHECKSUMS=$(bun_fetch "$checksum_url" 2>/dev/null || true)
    EXPECTED_SHA256=$(printf '%s\n' "$CHECKSUMS" |
      awk -v asset="$downloaded_asset" '$2 == asset || $2 == "*" asset { print tolower($1); exit }')
    if printf '%s' "$EXPECTED_SHA256" | grep -Eq '^[0-9a-f]{64}$'; then break; fi
    EXPECTED_SHA256=""
  done
  printf '%s' "$EXPECTED_SHA256" | grep -Eq '^[0-9a-f]{64}$' \
    || err "Could not find a trusted SHA-256 checksum for $downloaded_asset in $VERSION"
  ACTUAL_SHA256=$(sha256_file "$tmpdir/jcode.download") \
    || err "sha256sum, shasum, openssl, or bun is required to verify the download"
  [ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] \
    || err "SHA-256 verification failed for $downloaded_asset"
  info "Verified SHA-256: $downloaded_asset"
fi

# ---------------------------------------------------------------------------
# Install binary
# ---------------------------------------------------------------------------
INSTALL_STAGE="binary_install"
mkdir -p "$INSTALL_DIR" "$stable_dir" "$version_dir"

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
  err "No prebuilt asset found for $ARTIFACT in $VERSION"
fi

chmod +x "$dest_version_dir/$bin_name" 2>/dev/null || true

# Symlink stable -> versioned
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

# ---------------------------------------------------------------------------
# macOS quarantine / signing
# ---------------------------------------------------------------------------
if [ "$(uname -s)" = "Darwin" ]; then
  macos_ok=1
  for xattr_name in com.apple.quarantine com.apple.provenance; do
    xattr -dr "$xattr_name" "$dest_version_dir/$bin_name" 2>/dev/null || true
  done
  for candidate_path in "$launcher_path" "$stable_dir/$bin_name"; do
    [ -e "$candidate_path" ] || continue
    if [ -L "$candidate_path" ]; then
      link_target=$(readlink -f "$candidate_path" 2>/dev/null || readlink "$candidate_path" 2>/dev/null || true)
      [ -n "$link_target" ] && [ -e "$link_target" ] && candidate_path="$link_target"
    fi
    for xattr_name in com.apple.quarantine com.apple.provenance; do
      xattr -d "$xattr_name" "$candidate_path" 2>/dev/null || true
    done
    if command -v codesign >/dev/null 2>&1; then
      codesign --force --deep --sign - "$candidate_path" >/dev/null 2>&1 || macos_ok=0
    else
      macos_ok=0
    fi
  done
  if [ "$macos_ok" = "1" ]; then
    info "Cleared macOS quarantine/provenance xattrs and re-adhoc-signed launcher."
  else
    info "Cleared macOS xattrs; ad-hoc resign skipped (codesign unavailable)."
  fi
  if "$launcher_path" setup-launcher </dev/null >/dev/null 2>&1; then
    info "Installed macOS launcher and turn-notification broker."
  fi
fi

# ---------------------------------------------------------------------------
# PATH configuration (same rc-file logic as install.sh)
# ---------------------------------------------------------------------------
INSTALL_STAGE="path_configuration"
if [ "$IS_WINDOWS" = true ]; then
  win_install_dir=$(cygpath -w "$INSTALL_DIR" 2>/dev/null || echo "$INSTALL_DIR")
  if command -v powershell.exe >/dev/null 2>&1; then
    current_user_path=$(powershell.exe -NoProfile -NonInteractive -Command \
      "[Environment]::GetEnvironmentVariable('Path','User')" 2>/dev/null | tr -d '\r' || true)
    if ! echo "$current_user_path" | grep -qF "$win_install_dir"; then
      new_user_path="$win_install_dir;$current_user_path"
      JCODE_NEW_USER_PATH="$new_user_path" powershell.exe -NoProfile -NonInteractive -Command \
        '[Environment]::SetEnvironmentVariable("Path", $env:JCODE_NEW_USER_PATH, "User")' >/dev/null 2>&1 || true
    fi
  fi
  info "Added $win_install_dir to your user PATH."
else
  added_to=""
  _have() { command -v "$1" >/dev/null 2>&1; }
  ensure_posix_rc() {
    local rc="$1" create="$2"
    if [ ! -f "$rc" ]; then [ "$create" = "yes" ] || return 0; mkdir -p "$(dirname "$rc")"; fi
    if ! grep -qF "$INSTALL_DIR" "$rc" 2>/dev/null; then
      printf '\n# Added by jcode installer\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$rc"
      added_to="$added_to $rc"
    fi
  }
  ensure_fish_rc() {
    local create="$1"
    local rc="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
    if [ ! -f "$rc" ]; then [ "$create" = "yes" ] || return 0; mkdir -p "$(dirname "$rc")"; fi
    if ! grep -qF "$INSTALL_DIR" "$rc" 2>/dev/null; then
      { printf '\n# Added by jcode installer\nif not contains "%s" $PATH\n    set -gx PATH "%s" $PATH\nend\n' "$INSTALL_DIR" "$INSTALL_DIR"; } >> "$rc"
      added_to="$added_to $rc"
    fi
  }
  if _have zsh || [ "$(uname -s)" = "Darwin" ] || [ -f "$HOME/.zshenv" ] || [ -f "$HOME/.zshrc" ]; then
    ensure_posix_rc "$HOME/.zshenv" yes
  fi
  if _have bash || [ -f "$HOME/.bashrc" ] || [ -f "$HOME/.bash_profile" ]; then
    ensure_posix_rc "$HOME/.bashrc" yes
  fi
  ensure_posix_rc "$HOME/.profile" yes
  if _have fish || [ -f "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ]; then
    ensure_fish_rc yes
  fi
  for rc in "$HOME/.zshrc" "$HOME/.zprofile" "$HOME/.bash_profile"; do
    ensure_posix_rc "$rc" no
  done
  if [ -n "$added_to" ]; then info "Added $INSTALL_DIR to PATH in:$added_to"; fi
fi

info "jcode $VERSION installed successfully!"
if command -v jcode >/dev/null 2>&1; then
  info "Run 'jcode' to get started."
else
  printf '  Run: \033[1;32mexport PATH="%s:$PATH" && jcode\033[0m\n' "$INSTALL_DIR"
fi

persist_install_conversion_id
INSTALL_STAGE="complete"
INSTALL_SUCCEEDED=1
