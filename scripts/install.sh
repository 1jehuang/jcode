#!/usr/bin/env bash
set -euo pipefail

REPO="1jehuang/jcode"
RELEASE_METADATA_BASE="${JCODE_RELEASE_METADATA_BASE:-https://jcode.sh/releases}"
IS_WINDOWS=false
IS_TERMUX=false
INSTALL_STAGE="startup"
INSTALL_SUCCEEDED=0
INSTALL_OS="unknown"
INSTALL_ARCH="unknown"
INSTALL_VERSION="unknown"
INSTALL_CHANNEL="stable"
EXPLICIT_CHANNEL=false
tmpdir=""

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
err()  { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: install.sh [OPTIONS]

Options:
  --prerelease, --preview    Install the latest prerelease build, save the
                             prerelease update channel, and live-reload running
                             jcode sessions.
  --stable                   Install the latest stable release and save the
                             stable update channel.
  --channel <name>           Select update channel: prerelease, preview, or stable.
  --version <tag>            Install a specific release tag.
  -h, --help                 Show this help message.

Environment:
  JCODE_CHANNEL=prerelease   Same as --prerelease.
  JCODE_VERSION=<tag>        Same as --version <tag>.
EOF
}

parse_channel() {
  case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
    prerelease|pre-release|preview) printf 'preview' ;;
    stable|release) printf 'stable' ;;
    *) return 1 ;;
  esac
}

if [ -n "${JCODE_CHANNEL:-}" ]; then
  INSTALL_CHANNEL=$(parse_channel "$JCODE_CHANNEL") \
    || err "Unknown JCODE_CHANNEL: $JCODE_CHANNEL (expected prerelease, preview, or stable)"
  EXPLICIT_CHANNEL=true
fi

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prerelease|--preview)
      INSTALL_CHANNEL="preview"
      EXPLICIT_CHANNEL=true
      shift
      ;;
    --stable)
      INSTALL_CHANNEL="stable"
      EXPLICIT_CHANNEL=true
      shift
      ;;
    --channel)
      [ "$#" -ge 2 ] || err "--channel requires a value (prerelease, preview, or stable)"
      INSTALL_CHANNEL=$(parse_channel "$2") \
        || err "Unknown channel: $2 (expected prerelease, preview, or stable)"
      EXPLICIT_CHANNEL=true
      shift 2
      ;;
    --channel=*)
      val="${1#--channel=}"
      INSTALL_CHANNEL=$(parse_channel "$val") \
        || err "Unknown channel: $val (expected prerelease, preview, or stable)"
      EXPLICIT_CHANNEL=true
      shift
      ;;
    --version)
      [ "$#" -ge 2 ] || err "--version requires a release tag"
      JCODE_VERSION="$2"
      shift 2
      ;;
    --version=*)
      JCODE_VERSION="${1#--version=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      err "Unknown option: $1 (use --help for usage)"
      ;;
  esac
done

valid_release_tag() {
  printf '%s' "$1" | grep -Eq '^(v[0-9]+\.[0-9]+\.[0-9]+([+.-][[:alnum:].-]+)?|selfdev-[a-z0-9]+-[0-9a-f]{40})$'
}

is_release_channel_marker() {
  printf '%s' "$1" | grep -Eq '^(v?[0-9]+\.[0-9]+\.[0-9]+([+.-][[:alnum:].-]+)?|selfdev-[a-z0-9]+-[0-9a-f]{40}|main-[[:alnum:].-]+)$'
}

sha256_file() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print tolower($1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print tolower($1)}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file" | awk '{print tolower($NF)}'
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
  stage="$1"
  outcome="$2"
  failure_stage="${3:-}"
  [ "${JCODE_NO_TELEMETRY:-}" != "1" ] || return 0
  [ "${DO_NOT_TRACK:-}" != "1" ] || return 0
  valid_conversion_id || return 0

  payload=$(printf '{"id":"%s","event":"install_funnel","version":"%s","os":"%s","arch":"%s","conversion_id":"%s","stage":"%s","outcome":"%s","source":"installer","install_method":"shell","failure_stage":"%s"}' \
    "$JCODE_INSTALL_CONVERSION_ID" \
    "$(telemetry_value "$INSTALL_VERSION")" \
    "$(telemetry_value "$INSTALL_OS")" \
    "$(telemetry_value "$INSTALL_ARCH")" \
    "$JCODE_INSTALL_CONVERSION_ID" \
    "$(telemetry_value "$stage")" \
    "$(telemetry_value "$outcome")" \
    "$(telemetry_value "$failure_stage")")
  curl -fsS --max-time 2 -H 'Content-Type: application/json' \
    --data "$payload" https://telemetry.jcode.sh/v1/event >/dev/null 2>&1 || true
}

persist_install_conversion_id() {
  [ "${JCODE_NO_TELEMETRY:-}" != "1" ] || return 0
  [ "${DO_NOT_TRACK:-}" != "1" ] || return 0
  valid_conversion_id || return 0
  jcode_home="${JCODE_HOME:-$HOME/.jcode}"
  mkdir -p "$jcode_home" 2>/dev/null || return 0
  (umask 077; printf '%s\n' "$JCODE_INSTALL_CONVERSION_ID" > "$jcode_home/install_conversion_id") \
    2>/dev/null || return 0
  chmod 600 "$jcode_home/install_conversion_id" 2>/dev/null || true
}

install_exit() {
  status=$?
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

INSTALL_STAGE="platform_detection"
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
    WINDOWS_ARCH=""
    # Git for Windows may itself be an emulated x64 process on Windows ARM64,
    # making `uname -m` report x86_64. Prefer any ARM64 OS environment signal.
    for candidate in "${PROCESSOR_ARCHITEW6432:-}" "${PROCESSOR_ARCHITECTURE:-}" "$ARCH"; do
      case "$candidate" in
        aarch64|AARCH64|arm64|Arm64|ARM64) WINDOWS_ARCH="aarch64"; break ;;
      esac
    done
    if [ -z "$WINDOWS_ARCH" ]; then
      for candidate in "${PROCESSOR_ARCHITEW6432:-}" "${PROCESSOR_ARCHITECTURE:-}" "$ARCH"; do
        case "$candidate" in
          x86_64|X64|AMD64) WINDOWS_ARCH="x86_64"; break ;;
        esac
      done
    fi
    case "$WINDOWS_ARCH" in
      x86_64) ARTIFACT="jcode-windows-x86_64" ;;
      aarch64) ARTIFACT="jcode-windows-aarch64" ;;
      *) err "Unsupported Windows architecture: $ARCH" ;;
    esac
    ;;
  *)
    err "Unsupported OS: $OS (try building from source: https://github.com/$REPO)"
    ;;
esac

report_install_funnel "installer_start" "success" ""

if [ "$IS_WINDOWS" = true ]; then
  INSTALL_DIR="${JCODE_INSTALL_DIR:-$LOCALAPPDATA/jcode/bin}"
else
  INSTALL_DIR="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
fi

# Resolve latest release tag for the selected channel.
# Stable prefers GitHub's redirect with static jcode.sh metadata fallback so
# unauthenticated GitHub API rate limits are avoided. Prerelease queries
# GitHub's releases API and picks the newest published prerelease that carries
# both this platform's artifact and SHA256SUMS.
INSTALL_STAGE="release_lookup"
VERSION="${JCODE_VERSION:-}"

release_has_artifact() {
  tag="$1"
  sums=$(curl -fsSL --retry 2 --connect-timeout 10 \
    "https://github.com/$REPO/releases/download/$tag/SHA256SUMS" 2>/dev/null || true)
  for name in "$ARTIFACT.tar.gz" "$ARTIFACT" "$ARTIFACT.exe"; do
    if printf '%s\n' "$sums" | awk -v a="$name" '$2 == a || $2 == "*" a { f=1 } END { exit !f }' \
      && curl -fsSIL --retry 2 --connect-timeout 10 -o /dev/null \
        "https://github.com/$REPO/releases/download/$tag/$name" 2>/dev/null; then
      return 0
    fi
  done
  return 1
}

resolve_latest_prerelease_tag() {
  api_url="https://api.github.com/repos/$REPO/releases?per_page=100"
  gh_token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
  if [ -n "$gh_token" ]; then
    releases_json=$(curl -fsSL --retry 2 --connect-timeout 10 \
      -H 'Accept: application/vnd.github+json' \
      -H "Authorization: Bearer $gh_token" \
      "$api_url" 2>/dev/null || true)
  else
    releases_json=$(curl -fsSL --retry 2 --connect-timeout 10 \
      -H 'Accept: application/vnd.github+json' \
      "$api_url" 2>/dev/null || true)
  fi
  [ -n "$releases_json" ] || return 1

  if command -v python3 >/dev/null 2>&1; then
    selected=$(printf '%s' "$releases_json" | python3 -c '
import json, re, sys
artifact = sys.argv[1]
exe = sys.argv[2]
expected_assets = {f"{artifact}.tar.gz", f"{artifact}{exe}"}
selfdev_re = re.compile(r"^selfdev-[a-z0-9]+-[0-9a-f]{40}$")
semver_re = re.compile(r"^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-preview\.([1-9][0-9]*)$")

try:
    releases = json.load(sys.stdin)
except Exception:
    sys.exit(1)

best_key = None
best_tag = None
for rel in releases:
    if not isinstance(rel, dict):
        continue
    if rel.get("draft") or not rel.get("prerelease"):
        continue
    published = (rel.get("published_at") or "").strip()
    tag = (rel.get("tag_name") or "").strip()
    if not published or not tag:
        continue
    semver_match = semver_re.match(tag)
    if not (selfdev_re.match(tag) or semver_match):
        continue
    assets = rel.get("assets") or []
    has_bin = any(
        isinstance(a, dict)
        and a.get("name") in expected_assets
        and (a.get("size") or 0) > 0
        and bool(a.get("browser_download_url"))
        for a in assets
    )
    has_sums = any(
        isinstance(a, dict)
        and a.get("name") == "SHA256SUMS"
        and (a.get("size") or 0) > 0
        and bool(a.get("browser_download_url"))
        for a in assets
    )
    if not (has_bin and has_sums):
        continue
    semver_key = tuple(int(g) for g in semver_match.groups()) if semver_match else (0, 0, 0, 0)
    key = (published, semver_key)
    if best_key is None or key > best_key:
        best_key = key
        best_tag = tag

if best_tag:
    print(best_tag)
' "$ARTIFACT" "$([ "$IS_WINDOWS" = true ] && printf '.exe' || printf '')" 2>/dev/null || true)
    if [ -n "$selected" ]; then
      printf '%s\n' "$selected"
      return 0
    fi
    return 1
  fi

  candidate_tags=$(printf '%s\n' "$releases_json" | awk '
    /"tag_name":/ { gsub(/.*"tag_name": *"|".*/, ""); tag=$0; draft=0; pre=0 }
    /"draft": *true/ { draft=1 }
    /"prerelease": *true/ { pre=1 }
    /"published_at":/ {
      if (tag != "" && !draft && pre && $0 !~ /null/) print tag
      tag=""; draft=0; pre=0
    }
  ' | grep -E '^(selfdev-[a-z0-9]+-[0-9a-f]{40}|v[0-9]+\.[0-9]+\.[0-9]+-preview\.[1-9][0-9]*)$' || true)
  for tag in $candidate_tags; do
    if release_has_artifact "$tag"; then
      printf '%s\n' "$tag"
      return 0
    fi
  done
  return 1
}

if [ -z "$VERSION" ]; then
  if [ "$INSTALL_CHANNEL" = "preview" ]; then
    info "Looking up latest prerelease build for $ARTIFACT..."
    VERSION=$(resolve_latest_prerelease_tag || true)
    valid_release_tag "$VERSION" \
      || err "No published prerelease build is available for $ARTIFACT yet. Check https://github.com/$REPO/releases or run without --prerelease for stable."
  else
    METADATA_VERSION=$(curl -fsSL --retry 2 --connect-timeout 10 \
      "$RELEASE_METADATA_BASE/latest/version" 2>/dev/null | tr -d '\r\n' || true)
    LATEST_RELEASE_URL=$(curl -fsSIL --retry 2 --connect-timeout 10 \
      -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null || true)
    case "$LATEST_RELEASE_URL" in
      */releases/tag/*) GITHUB_VERSION="${LATEST_RELEASE_URL##*/}" ;;
      *) GITHUB_VERSION="" ;;
    esac
    if valid_release_tag "$GITHUB_VERSION"; then
      VERSION="$GITHUB_VERSION"
    elif valid_release_tag "$METADATA_VERSION"; then
      VERSION="$METADATA_VERSION"
      info "GitHub release lookup unavailable; using cached jcode.sh metadata ($VERSION)."
    fi
  fi
fi
valid_release_tag "$VERSION" || err "Failed to determine latest version"

# Release assets publish asynchronously per platform, so the latest stable
# release may briefly lack this platform's binary or its SHA256SUMS entry.
# When the version was not pinned on stable, fall back to the newest stable
# release that has both.
if [ -z "${JCODE_VERSION:-}" ] && [ "$INSTALL_CHANNEL" != "preview" ] && ! release_has_artifact "$VERSION"; then
  info "Release $VERSION has no published $ARTIFACT asset yet; looking for the newest release that does..."
  EXISTING_SEMVER=""
  for existing_bin in "$INSTALL_DIR/jcode" "$INSTALL_DIR/jcode.exe"; do
    if [ -x "$existing_bin" ]; then
      EXISTING_SEMVER=$("$existing_bin" --version 2>/dev/null | grep -Eo '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
      break
    fi
  done
  FOUND_FALLBACK=""
  for page in 1 2 3 4 5; do
    FALLBACK_TAGS=$(curl -fsSL --retry 2 --connect-timeout 10 \
      "https://api.github.com/repos/$REPO/releases?per_page=100&page=$page" 2>/dev/null |
      awk '
        /"tag_name":/ { gsub(/.*"tag_name": *"|".*/, ""); tag=$0 }
        /"draft": *true/ { draft=1 }
        /"prerelease":/ { if ($0 ~ /true/ || draft) skip=1; if (tag != "" && !skip) print tag; tag=""; draft=0; skip=0 }
      ' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -t. -k1.2,1nr -k2,2nr -k3,3nr || true)
    [ -z "$FALLBACK_TAGS" ] && break
    REACHED_EXISTING=false
    for tag in $FALLBACK_TAGS; do
      [ "$tag" = "$VERSION" ] && continue
      if [ -n "$EXISTING_SEMVER" ]; then
        # Never downgrade an existing install via fallback.
        newest=$(printf '%s\n%s\n' "${tag#v}" "$EXISTING_SEMVER" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)
        if [ "$newest" = "$EXISTING_SEMVER" ]; then
          REACHED_EXISTING=true
          break
        fi
      fi
      if release_has_artifact "$tag"; then
        info "Using $tag (newest release with a published $ARTIFACT asset)."
        VERSION="$tag"
        FOUND_FALLBACK="$tag"
        break
      fi
    done
    [ -n "$FOUND_FALLBACK" ] && break
    [ "$REACHED_EXISTING" = true ] && break
  done
fi
INSTALL_VERSION="${VERSION#v}"

GITHUB_RELEASE_BASE="https://github.com/$REPO/releases/download/$VERSION"

if [ "$IS_WINDOWS" = true ]; then
  EXE=".exe"
  builds_dir="$LOCALAPPDATA/jcode/builds"
else
  EXE=""
  builds_dir="${JCODE_HOME:-$HOME/.jcode}/builds"
fi
stable_dir="$builds_dir/stable"
current_dir="$builds_dir/current"
shared_server_dir="$builds_dir/shared-server"
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

INSTALL_STAGE="artifact_download"
download_mode=""
downloaded_asset=""
DOWNLOAD_BASES=$(curl -fsSL --retry 2 --connect-timeout 10 \
  "$RELEASE_METADATA_BASE/$VERSION/download-bases" 2>/dev/null || true)
DOWNLOAD_BASES=$(printf '%s\n%s\n' "$DOWNLOAD_BASES" "$GITHUB_RELEASE_BASE" |
  awk '/^https:\/\/[^[:space:]]+$/ && !seen[$0]++')

for candidate in "$ARTIFACT.tar.gz" "$ARTIFACT$EXE"; do
  while IFS= read -r base; do
    [ -n "$base" ] || continue
    if curl -fsSL --retry 2 --connect-timeout 10 \
      "${base%/}/$candidate" -o "$tmpdir/jcode.download" 2>/dev/null; then
      downloaded_asset="$candidate"
      case "$candidate" in
        *.tar.gz) download_mode="tar" ;;
        *) download_mode="bin" ;;
      esac
      break 2
    fi
  done <<EOF
$DOWNLOAD_BASES
EOF
done

if [ -n "$download_mode" ]; then
  INSTALL_STAGE="artifact_verification"
  EXPECTED_SHA256=""
  for checksum_url in \
    "$RELEASE_METADATA_BASE/$VERSION/SHA256SUMS" \
    "$GITHUB_RELEASE_BASE/SHA256SUMS"; do
    CHECKSUMS=$(curl -fsSL --retry 2 --connect-timeout 10 \
      "$checksum_url" 2>/dev/null || true)
    EXPECTED_SHA256=$(printf '%s\n' "$CHECKSUMS" |
      awk -v asset="$downloaded_asset" '$2 == asset || $2 == "*" asset { print tolower($1); exit }')
    if printf '%s' "$EXPECTED_SHA256" | grep -Eq '^[0-9a-f]{64}$'; then
      break
    fi
    EXPECTED_SHA256=""
  done
  printf '%s' "$EXPECTED_SHA256" | grep -Eq '^[0-9a-f]{64}$' \
    || err "Could not find a trusted SHA-256 checksum for $downloaded_asset in $VERSION"
  ACTUAL_SHA256=$(sha256_file "$tmpdir/jcode.download") \
    || err "sha256sum, shasum, or openssl is required to verify the download"
  [ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] \
    || err "SHA-256 verification failed for $downloaded_asset"
  info "Verified SHA-256: $downloaded_asset"
fi

INSTALL_STAGE="binary_install"
mkdir -p "$INSTALL_DIR" "$stable_dir" "$current_dir" "$shared_server_dir" "$version_dir"

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
find "$dest_version_dir" -maxdepth 1 -type f -name '*.bin' -exec chmod +x {} + 2>/dev/null || true
# Stamp every installed file with a uniform current mtime so freshness checks
# treat the newly installed release as strictly newer than an older daemon.
find "$dest_version_dir" -maxdepth 1 -type f -exec touch {} + 2>/dev/null || true

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

stable_prev=""
[ -f "$builds_dir/stable-version" ] && stable_prev=$(tr -d '\r\n' < "$builds_dir/stable-version" 2>/dev/null || true)
shared_prev=""
[ -f "$builds_dir/shared-server-version" ] && shared_prev=$(tr -d '\r\n' < "$builds_dir/shared-server-version" 2>/dev/null || true)

should_advance_shared_server=false
if [ ! -e "$shared_server_dir/$bin_name" ]; then
  should_advance_shared_server=true
elif [ -n "$shared_prev" ] && [ "$shared_prev" = "$stable_prev" ]; then
  should_advance_shared_server=true
elif [ -n "$shared_prev" ] && is_release_channel_marker "$shared_prev"; then
  should_advance_shared_server=true
fi

if [ "$IS_WINDOWS" = true ]; then
  if [ "$should_advance_shared_server" = true ]; then
    cp -f "$dest_version_dir/$bin_name" "$shared_server_dir/$bin_name"
    printf '%s\n' "$version" > "$builds_dir/shared-server-version"
  fi
  cp -f "$dest_version_dir/$bin_name" "$stable_dir/$bin_name"
  printf '%s\n' "$version" > "$builds_dir/stable-version"
  cp -f "$dest_version_dir/$bin_name" "$current_dir/$bin_name"
  printf '%s\n' "$version" > "$builds_dir/current-version"
  cp -f "$stable_dir/$bin_name" "$launcher_path"
else
  if [ "$should_advance_shared_server" = true ]; then
    ln -sfn "$dest_version_dir/$bin_name" "$shared_server_dir/$bin_name"
    printf '%s\n' "$version" > "$builds_dir/shared-server-version"
  fi
  ln -sfn "$dest_version_dir/$bin_name" "$stable_dir/$bin_name"
  printf '%s\n' "$version" > "$builds_dir/stable-version"
  ln -sfn "$dest_version_dir/$bin_name" "$current_dir/$bin_name"
  printf '%s\n' "$version" > "$builds_dir/current-version"
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

persist_update_channel() {
  target_channel="$1"
  cfg_home="${JCODE_HOME:-$HOME/.jcode}"
  cfg_file="$cfg_home/config.toml"
  if [ -x "$launcher_path" ] \
    && "$launcher_path" --no-update update --channel "$target_channel" --set-channel-only </dev/null >/dev/null 2>&1 \
    && [ -f "$cfg_file" ] \
    && grep -Eq '^[[:space:]]*update_channel[[:space:]]*=[[:space:]]*"'"$target_channel"'"' "$cfg_file"; then
    return 0
  fi
  mkdir -p "$cfg_home" 2>/dev/null || return 1
  if [ ! -f "$cfg_file" ]; then
    printf '[features]\nupdate_channel = "%s"\n' "$target_channel" > "$cfg_file"
    return 0
  fi
  cfg_tmp="$tmpdir/config.toml.tmp"
  awk -v ch="$target_channel" '
    BEGIN { in_features=0; has_features=0; replaced=0 }
    /^\[features\][[:space:]]*(#.*)?$/ {
      in_features=1; has_features=1; print; next
    }
    /^\[/ {
      if (in_features && !replaced) {
        print "update_channel = \"" ch "\""
        replaced=1
      }
      in_features=0
      print
      next
    }
    {
      if (in_features && $0 ~ /^[[:space:]]*update_channel[[:space:]]*=/) {
        print "update_channel = \"" ch "\""
        replaced=1
        next
      }
      print
    }
    END {
      if (in_features && !replaced) {
        print "update_channel = \"" ch "\""
      } else if (!has_features) {
        print ""
        print "[features]"
        print "update_channel = \"" ch "\""
      }
    }
  ' "$cfg_file" > "$cfg_tmp" && mv "$cfg_tmp" "$cfg_file"
}

if [ "$EXPLICIT_CHANNEL" = true ]; then
  if persist_update_channel "$INSTALL_CHANNEL"; then
    if [ "$INSTALL_CHANNEL" = "preview" ]; then
      info "Saved update channel: preview (automatic live updates will follow prereleases)."
    else
      info "Saved update channel: stable."
    fi
  fi
fi

if [ "$(uname -s)" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$dest_version_dir/$bin_name" 2>/dev/null || true
  # Generate the architecture-matched, faceless notification broker from the
  # verified binary and remove legacy CLI launcher bundles from ~/Applications.
  # Jcode Desktop is the only Spotlight/Launchpad entry. Best-effort here: the
  # first interactive jcode launch performs the same version-gated repair.
  if "$launcher_path" setup-launcher </dev/null >/dev/null 2>&1; then
    info "Installed macOS turn-notification helper."
  fi
fi

hotkey_setup_ready=false
case "$(uname -s)" in
Darwin|Linux)
  if "$launcher_path" setup-hotkey </dev/null >/dev/null 2>&1; then
    hotkey_setup_ready=true
  fi
  ;;
esac

# Retire any background server still running the old binary so the freshly
# installed version is picked up without the user having to kill a daemon by
# hand (issue #291). We use the graceful `server reload` path, which hands live
# headless/swarm sessions to a newly-exec'd server instead of dropping them, and
# only reloads when the running server is genuinely older than what we just
# installed (so a newer/dev daemon is never downgraded). This is best-effort:
# it must never fail the install, and it is skipped when no server is running.
INSTALL_STAGE="server_reload"
if [ "${JCODE_SKIP_SERVER_RELOAD:-}" != "1" ]; then
  reload_bin="$launcher_path"
  [ -x "$reload_bin" ] || reload_bin="$stable_dir/$bin_name"
  if [ -x "$reload_bin" ]; then
    if [ "$EXPLICIT_CHANNEL" = true ]; then
      reload_args=(--no-update server reload --force)
    else
      reload_args=(--no-update server reload)
    fi
    if "$reload_bin" "${reload_args[@]}" </dev/null >/dev/null 2>&1; then
      info "Live-reloaded running jcode server and connected sessions onto $VERSION (if active)."
    fi
  fi
fi

if [ "$IS_WINDOWS" = true ]; then
  INSTALL_STAGE="path_configuration"
  win_install_dir=$(cygpath -w "$INSTALL_DIR" 2>/dev/null || echo "$INSTALL_DIR")

  # Persist the launcher dir on the USER PATH so every future shell (PowerShell,
  # cmd, Git Bash, Windows Terminal) finds jcode without manual setup. This is
  # the Git Bash (`curl | sh`) counterpart of install.ps1's Set-JcodeUserPath:
  # read the user PATH, drop stale jcode launcher entries (case- and trailing-
  # slash-insensitive), prepend the canonical dir, and broadcast
  # WM_SETTINGCHANGE so already-open apps can pick up the change.
  win_path_persisted=false
  _win_path_key() { printf '%s' "$1" | sed 's|[\\/]*$||' | tr '[:upper:]' '[:lower:]'; }
  if command -v powershell.exe >/dev/null 2>&1; then
    current_user_path=$(powershell.exe -NoProfile -NonInteractive -Command \
      "[Environment]::GetEnvironmentVariable('Path','User')" 2>/dev/null | tr -d '\r' || true)
    target_key=$(_win_path_key "$win_install_dir")
    new_user_path="$win_install_dir"
    set -f
    IFS=';'
    for entry in $current_user_path; do
      [ -n "$entry" ] || continue
      [ "$(_win_path_key "$entry")" = "$target_key" ] && continue
      new_user_path="$new_user_path;$entry"
    done
    unset IFS
    set +f
    if [ "$new_user_path" = "$current_user_path" ]; then
      win_path_persisted=true
    elif JCODE_NEW_USER_PATH="$new_user_path" powershell.exe -NoProfile -NonInteractive -Command \
      '[Environment]::SetEnvironmentVariable("Path", $env:JCODE_NEW_USER_PATH, "User")' >/dev/null 2>&1; then
      win_path_persisted=true
      # Broadcast WM_SETTINGCHANGE (0x001A) with the "Environment" lParam to
      # HWND_BROADCAST so running shells learn about the new PATH. Best-effort.
      # The script lives in a quoted heredoc so no bash expansion touches it;
      # setup_friction_eval.sh parse-checks this exact block with real pwsh.
      win_broadcast_ps=$(cat <<'JCODE_PS_BROADCAST_EOF'
$sig = '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
$type = Add-Type -MemberDefinition $sig -Name 'JcodeEnvBroadcast' -Namespace Win32 -PassThru
[UIntPtr]$result = [UIntPtr]::Zero
$type::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result) | Out-Null
JCODE_PS_BROADCAST_EOF
)
      powershell.exe -NoProfile -NonInteractive -Command "$win_broadcast_ps" >/dev/null 2>&1 || true
    fi
  fi

  echo ""
  info "✅ jcode $VERSION installed successfully!"
  echo ""
  if [ "$win_path_persisted" = true ]; then
    info "Added $win_install_dir to your user PATH. New terminals will find jcode automatically."
  fi
  if command -v jcode >/dev/null 2>&1; then
    info "Run 'jcode' to get started."
  else
    echo "  To start using jcode in THIS terminal right now, run:"
    echo ""
    printf '    \033[1;32mexport PATH="%s:$PATH" && jcode\033[0m\n' "$INSTALL_DIR"
    if [ "$win_path_persisted" != true ]; then
      echo ""
      echo "  To add jcode to PATH permanently (PowerShell):"
      echo ""
      printf '    \033[1;32m[Environment]::SetEnvironmentVariable("Path", "%s;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")\033[0m\n' "$win_install_dir"
    fi
  fi
else
  INSTALL_STAGE="path_configuration"
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
    if [ "$hotkey_setup_ready" = true ]; then
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

persist_install_conversion_id
INSTALL_STAGE="complete"
INSTALL_SUCCEEDED=1
