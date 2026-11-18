#!/usr/bin/env bash
set -euo pipefail

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export JCODE_TEST_PUBLISH_HELPER="$repo_dir/scripts/lib/test_publish_build.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/home" "$tmp/install"

cat > "$tmp/bin/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) printf '%s\n' "${TEST_UNAME_S:-Linux}" ;;
  -m) printf '%s\n' "${TEST_UNAME_M:-x86_64}" ;;
  *) printf '%s\n' "${TEST_UNAME_S:-Linux}" ;;
esac
EOF

cat > "$tmp/bin/curl" <<'EOF'
#!/usr/bin/env bash
output=""
payload=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    --data) payload="$2"; shift 2 ;;
    http*) url="$1"; shift ;;
    *) shift ;;
  esac
done
[ -z "${DOWNLOAD_URL_LOG:-}" ] || printf '%s\n' "$url" >> "$DOWNLOAD_URL_LOG"
case "$url" in
  *telemetry.jcode.sh*) printf '%s\n' "$payload" >> "$INSTALL_TELEMETRY_LOG" ;;
  *jcode.sh/releases/latest/version)
    [ "${FAIL_RELEASE:-0}" != "1" ] || exit 22
    [ "${FAIL_METADATA_RELEASE:-0}" != "1" ] || exit 22
    printf 'v1.2.3\n'
    ;;
  *jcode.sh/releases/v1.2.3/download-bases)
    printf 'https://mirror.invalid/releases/v1.2.3\n'
    printf 'https://github.com/1jehuang/jcode/releases/download/v1.2.3\n'
    ;;
  *jcode.sh/releases/v1.2.3/SHA256SUMS)
    if [ "${METADATA_CHECKSUM_HTML:-0}" = "1" ]; then
      printf '<!doctype html><title>fallback page</title>\n'
      exit 0
    fi
    checksum='8d57abb57a0dae3ff23c8f0df1f51951b7772822e0d560e860d6f68c24ef6d3d'
    [ "${BAD_CHECKSUM:-0}" != "1" ] || checksum='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    printf '%s  %s\n' "$checksum" "${TEST_CHECKSUM_ASSET:-jcode-linux-x86_64.tar.gz}"
    ;;
  *github.com*/releases/download/v1.2.3/SHA256SUMS)
    checksum='8d57abb57a0dae3ff23c8f0df1f51951b7772822e0d560e860d6f68c24ef6d3d'
    [ "${BAD_CHECKSUM:-0}" != "1" ] || checksum='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    printf '%s  %s\n' "$checksum" "${TEST_CHECKSUM_ASSET:-jcode-linux-x86_64.tar.gz}"
    ;;
  *github.com*/releases/latest)
    [ "${FAIL_RELEASE:-0}" != "1" ] || exit 22
    [ "${FAIL_GITHUB_RELEASE:-0}" != "1" ] || exit 22
    printf 'https://github.com/1jehuang/jcode/releases/tag/v1.2.3'
    ;;
  *mirror.invalid*) exit 22 ;;
  *github.com*/releases/download/*)
    [ -n "$output" ] || exit 2
    printf 'fake archive' > "$output"
    ;;
  *) exit 2 ;;
esac
EOF

cat > "$tmp/bin/tar" <<'EOF'
#!/usr/bin/env bash
dest=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -C) dest="$2"; shift 2 ;;
    *) shift ;;
  esac
done
artifact="${TEST_ARCHIVE_ARTIFACT:-jcode-linux-x86_64}"
cat > "$dest/$artifact" <<'BIN'
#!/usr/bin/env bash
if [ "${1:-}" = "--version" ]; then printf 'jcode 1.2.3\n'; exit 0; fi
if [ "${2:-}" = publish-build ]; then exec bash "$JCODE_TEST_PUBLISH_HELPER" "$@"; fi
if [ "${1:-}" = "setup-hotkey" ] && [ -n "${HOTKEY_SETUP_LOG:-}" ]; then
  printf '%s\n' "$*" >> "$HOTKEY_SETUP_LOG"
fi
BIN
chmod +x "$dest/$artifact"
EOF
chmod +x "$tmp/bin/uname" "$tmp/bin/curl" "$tmp/bin/tar"

conversion_id="11111111-2222-4333-8444-555555555555"
telemetry_log="$tmp/telemetry.jsonl"
hotkey_setup_log="$tmp/hotkey-setup.log"
PATH="$tmp/bin:$PATH" \
HOME="$tmp/home" \
JCODE_HOME="$tmp/home/.jcode" \
JCODE_INSTALL_DIR="$tmp/install" \
JCODE_INSTALL_CONVERSION_ID="$conversion_id" \
JCODE_SKIP_SERVER_RELOAD=1 \
INSTALL_TELEMETRY_LOG="$telemetry_log" \
HOTKEY_SETUP_LOG="$hotkey_setup_log" \
bash "$repo_dir/scripts/install.sh" >/dev/null

test "$(cat "$tmp/home/.jcode/install_conversion_id")" = "$conversion_id"
grep -q '"stage":"installer_start".*"outcome":"success"' "$telemetry_log"
grep -q '"stage":"installer_finish".*"outcome":"success"' "$telemetry_log"
test "$(cat "$hotkey_setup_log")" = "setup-hotkey"

# If GitHub's release page is blocked, the static jcode.sh version endpoint
# must keep the complete install path working.
PATH="$tmp/bin:$PATH" \
HOME="$tmp/home-metadata-fallback" \
JCODE_HOME="$tmp/home-metadata-fallback/.jcode" \
JCODE_INSTALL_DIR="$tmp/install-metadata-fallback" \
JCODE_SKIP_SERVER_RELOAD=1 \
JCODE_NO_TELEMETRY=1 \
FAIL_GITHUB_RELEASE=1 \
bash "$repo_dir/scripts/install.sh" >/dev/null
test -x "$tmp/install-metadata-fallback/jcode"

# A static host may return its HTML fallback with HTTP 200 for a missing path.
# Treat that as invalid metadata and continue to GitHub's checksum file.
PATH="$tmp/bin:$PATH" \
HOME="$tmp/home-checksum-fallback" \
JCODE_HOME="$tmp/home-checksum-fallback/.jcode" \
JCODE_INSTALL_DIR="$tmp/install-checksum-fallback" \
JCODE_SKIP_SERVER_RELOAD=1 \
JCODE_NO_TELEMETRY=1 \
METADATA_CHECKSUM_HTML=1 \
bash "$repo_dir/scripts/install.sh" >/dev/null
test -x "$tmp/install-checksum-fallback/jcode"

# Git for Windows can be x64-emulated on Windows ARM64. In that case uname -m
# reports x86_64 while PROCESSOR_ARCHITEW6432 exposes the native ARM64 OS.
windows_url_log="$tmp/windows-arm64-urls.log"
PATH="$tmp/bin:$PATH" \
HOME="$tmp/home-windows-arm64" \
LOCALAPPDATA="$tmp/localappdata-windows-arm64" \
JCODE_HOME="$tmp/home-windows-arm64/.jcode" \
JCODE_INSTALL_DIR="$tmp/install-windows-arm64" \
JCODE_SKIP_SERVER_RELOAD=1 \
JCODE_NO_TELEMETRY=1 \
TEST_UNAME_S=MINGW64_NT-10.0 \
TEST_UNAME_M=x86_64 \
PROCESSOR_ARCHITECTURE=AMD64 \
PROCESSOR_ARCHITEW6432=ARM64 \
TEST_ARCHIVE_ARTIFACT=jcode-windows-aarch64.exe \
TEST_CHECKSUM_ASSET=jcode-windows-aarch64.tar.gz \
DOWNLOAD_URL_LOG="$windows_url_log" \
bash "$repo_dir/scripts/install.sh" >/dev/null
grep -q '/jcode-windows-aarch64.tar.gz$' "$windows_url_log"
test -x "$tmp/install-windows-arm64/jcode.exe"

failure_log="$tmp/failure.jsonl"
if PATH="$tmp/bin:$PATH" \
  HOME="$tmp/home-failure" \
  JCODE_HOME="$tmp/home-failure/.jcode" \
  JCODE_INSTALL_DIR="$tmp/install-failure" \
  JCODE_INSTALL_CONVERSION_ID="$conversion_id" \
  JCODE_SKIP_SERVER_RELOAD=1 \
  INSTALL_TELEMETRY_LOG="$failure_log" \
  FAIL_RELEASE=1 \
  bash "$repo_dir/scripts/install.sh" >/dev/null 2>&1; then
  echo "expected release lookup failure" >&2
  exit 1
fi
grep -q '"stage":"installer_finish".*"outcome":"failure".*"failure_stage":"release_lookup"' "$failure_log"

checksum_failure_log="$tmp/checksum-failure.jsonl"
if PATH="$tmp/bin:$PATH" \
  HOME="$tmp/home-checksum-failure" \
  JCODE_HOME="$tmp/home-checksum-failure/.jcode" \
  JCODE_INSTALL_DIR="$tmp/install-checksum-failure" \
  JCODE_INSTALL_CONVERSION_ID="$conversion_id" \
  JCODE_SKIP_SERVER_RELOAD=1 \
  INSTALL_TELEMETRY_LOG="$checksum_failure_log" \
  BAD_CHECKSUM=1 \
  bash "$repo_dir/scripts/install.sh" >/dev/null 2>&1; then
  echo "expected checksum verification failure" >&2
  exit 1
fi
grep -q '"stage":"installer_finish".*"outcome":"failure".*"failure_stage":"artifact_verification"' "$checksum_failure_log"

if grep -q 'api.github.com' "$windows_url_log"; then
  echo "installer must not depend on the rate-limited unauthenticated GitHub API" >&2
  exit 1
fi

privacy_log="$tmp/privacy.jsonl"
PATH="$tmp/bin:$PATH" \
HOME="$tmp/home-private" \
JCODE_HOME="$tmp/home-private/.jcode" \
JCODE_INSTALL_DIR="$tmp/install-private" \
JCODE_INSTALL_CONVERSION_ID="$conversion_id" \
JCODE_SKIP_SERVER_RELOAD=1 \
JCODE_NO_TELEMETRY=1 \
INSTALL_TELEMETRY_LOG="$privacy_log" \
bash "$repo_dir/scripts/install.sh" >/dev/null
test ! -e "$privacy_log"
test ! -e "$tmp/home-private/.jcode/install_conversion_id"

echo "installer conversion telemetry tests passed"
