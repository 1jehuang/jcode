#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fixture="$tmp/repo"; fake_bin="$tmp/bin"
mkdir -p "$fixture/scripts/lib" "$fake_bin"
cp "$repo_root/scripts/install_release.sh" "$fixture/scripts/"
printf '%s\n' 'jcode_configure_path() { :; }' > "$fixture/scripts/lib/configure_path.sh"
printf '%s\n' '[workspace]' > "$fixture/Cargo.toml"
cat > "$fake_bin/git" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  *"rev-parse --show-toplevel"*) printf '%s\n' "$TEST_REPO" ;;
  *"rev-parse --git-dir"*) printf '%s\n' .git ;;
  *"rev-parse --short HEAD"*) printf '%s\n' abc123def ;;
  *"log -1 --format=%ci"*) printf '%s\n' '2026-08-09 20:00:00 +0000' ;;
  *"status --porcelain"*) [[ "${TEST_DIRTY:-0}" == 1 ]] && printf '%s\n' ' M file' ;;
  *) exit 1 ;;
esac
EOF
cat > "$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
test "${JCODE_BUILD_GIT_HASH:-}" = abc123def
test "${JCODE_BUILD_GIT_DATE:-}" = '2026-08-09 20:00:00 +0000'
test "${JCODE_BUILD_GIT_DIRTY:-}" = "${TEST_DIRTY:-0}"
mkdir -p "$TEST_REPO/target/release"
cat > "$TEST_REPO/target/release/jcode" <<BIN
#!/usr/bin/env bash
if [[ "\${1:-}" == --version ]]; then printf '%s\\n' 'jcode v0.0.0-dev (${TEST_BINARY_HASH:-$JCODE_BUILD_GIT_HASH}, dirty)'; fi
BIN
chmod +x "$TEST_REPO/target/release/jcode"
EOF
chmod +x "$fake_bin/git" "$fake_bin/cargo"
run_install() {
  HOME="$tmp/home" PATH="$fake_bin:/usr/bin:/bin" TEST_REPO="$fixture" JCODE_RELEASE_PROFILE=release \
    JCODE_INSTALL_DIR="$tmp/launcher" JCODE_SKIP_SERVER_RELOAD=1 "$fixture/scripts/install_release.sh"
}
TEST_DIRTY=1 run_install >/dev/null
test -x "$tmp/home/.jcode/builds/versions/abc123def-dirty/jcode"
test "$(cat "$tmp/home/.jcode/builds/current-version")" = abc123def-dirty
rm -rf "$tmp/home" "$tmp/launcher"
if TEST_BINARY_HASH=stale999 TEST_DIRTY=0 run_install >"$tmp/out" 2>"$tmp/err"; then
  echo "installer accepted a binary with stale metadata" >&2; exit 1
fi
grep -Fq 'does not report expected git identity: (abc123def)' "$tmp/err"
test ! -e "$tmp/home/.jcode/builds/current-version"
echo "install_release metadata regression test passed"
