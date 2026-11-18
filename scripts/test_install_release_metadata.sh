#!/usr/bin/env bash
set -euo pipefail
# The harness may export a Cargo wrapper function. This test must invoke only
# its fake executable, never host build/remote-build policy or logging hooks.
unset -f cargo 2>/dev/null || true
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export JCODE_TEST_PUBLISH_HELPER="$repo_root/scripts/lib/test_publish_build.sh"
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
# payload variant: ${TEST_PAYLOAD:-one}
if [[ "\${1:-}" == --version ]]; then printf '%s\\n' 'jcode v0.0.0-dev (${TEST_BINARY_HASH:-$JCODE_BUILD_GIT_HASH}, dirty)'; exit 0; fi
if [[ "\${2:-}" == publish-build ]]; then exec bash "\$JCODE_TEST_PUBLISH_HELPER" "\$@"; fi
BIN
chmod +x "$TEST_REPO/target/release/jcode"
EOF
chmod +x "$fake_bin/git" "$fake_bin/cargo"
run_install() {
  HOME="$tmp/home" JCODE_HOME="$tmp/home/.jcode" PATH="$fake_bin:/usr/bin:/bin" \
    TEST_REPO="$fixture" JCODE_RELEASE_PROFILE=release \
    JCODE_INSTALL_DIR="$tmp/launcher" JCODE_SKIP_SERVER_RELOAD=1 JCODE_SKIP_TARGET_SWEEP=1 \
    JCODE_KEEP_BUILDS=0 "$fixture/scripts/install_release.sh"
}
TEST_DIRTY=1 run_install >/dev/null
first="$(cat "$tmp/home/.jcode/builds/current-version")"
[[ "$first" =~ ^abc123def-dirty-[0-9a-f]{64}$ ]]
first_binary="$tmp/home/.jcode/builds/versions/$first/jcode"
cp "$first_binary" "$tmp/first-payload"
TEST_DIRTY=1 TEST_PAYLOAD=two run_install >/dev/null
second="$(cat "$tmp/home/.jcode/builds/current-version")"
[[ "$second" =~ ^abc123def-dirty-[0-9a-f]{64}$ && "$second" != "$first" ]]
cmp "$first_binary" "$tmp/first-payload"
test -x "$tmp/home/.jcode/builds/versions/$second/jcode"
TEST_DIRTY=1 TEST_PAYLOAD=two run_install >/dev/null
test "$(cat "$tmp/home/.jcode/builds/current-version")" = "$second"
test "$(find "$tmp/home/.jcode/builds/versions" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" = 2
rm -rf "$tmp/home" "$tmp/launcher"
if TEST_BINARY_HASH=stale999 TEST_DIRTY=0 run_install >"$tmp/out" 2>"$tmp/err"; then
  echo "installer accepted a binary with stale metadata" >&2; exit 1
fi
grep -Fq 'does not report expected git identity: (abc123def)' "$tmp/err"
test ! -e "$tmp/home/.jcode/builds/current-version"
echo "install_release metadata regression test passed"
