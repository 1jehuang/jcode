#!/usr/bin/env bash
# Build and upload a macOS arm64 (aarch64-apple-darwin) release binary directly
# using the hosted justrust macOS worker, bypassing GitHub Actions runners.
# Usage: publish_macos_release_direct.sh <40-char-sha> [tag]
set -euo pipefail

sha=${1:?usage: publish_macos_release_direct.sh <40-char-sha> [tag]}
tag=${2-}

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

log_dir="${JCODE_HOME:-$HOME/.jcode}/logs"
mkdir -p "$log_dir"
log_file="$log_dir/macos-release.log"
status_file="$log_dir/macos-release-status.json"
exec >>"$log_file" 2>&1

echo "=== $(date -Iseconds) macos-release start sha=$sha tag=${tag:-auto} ==="

current_step="init"
publish_state="running"
publish_detail=""
work=""

write_status() {
  local tmp="${status_file}.tmp.$$"
  if command -v jq >/dev/null 2>&1; then
    jq -n \
      --arg sha "$sha" \
      --arg tag "$tag" \
      --arg state "$publish_state" \
      --arg step "$current_step" \
      --arg detail "$publish_detail" \
      --arg updated_at "$(date -Iseconds)" \
      --arg log_file "$log_file" \
      '{
        sha: $sha,
        tag: (if $tag == "" then null else $tag end),
        state: $state,
        step: $step,
        detail: (if $detail == "" then null else $detail end),
        updated_at: $updated_at,
        log_file: $log_file
      }' >"$tmp" 2>/dev/null && mv -f "$tmp" "$status_file" || rm -f "$tmp"
  fi
}

on_exit() {
  local rc=$?
  if [ -n "$work" ]; then
    rm -rf -- "$work"
  fi
  if [ "$rc" -ne 0 ] && [ "$publish_state" = "running" ]; then
    publish_state="failed"
    if [ -z "$publish_detail" ] && [ -f "$log_file" ]; then
      publish_detail=$(tail -n 5 "$log_file" | grep -v '^=== ' | tail -n 1 || true)
    fi
    write_status
  fi
}
trap on_exit EXIT

write_status

lock_dir="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}"
mkdir -p "$lock_dir"
exec 9>"$lock_dir/jcode-macos-release.lock"
flock 9

export GH_HOST=github.com
export GH_PROMPT_DISABLED=1
repo="1jehuang/jcode"
target="aarch64-apple-darwin"
artifact="jcode-macos-aarch64"
archive="${artifact}.tar.gz"

current_step="resolve_tag"
if [ -z "$tag" ]; then
  tag=$(git tag --points-at "$sha" --list 'v[0-9]*.[0-9]*.[0-9]*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)
fi
if [ -z "$tag" ]; then
  tag=$(gh api "repos/$repo/releases?per_page=100" | jq -r \
    --arg sha "$sha" '
      [.[] | select((.prerelease | not) and .target_commitish == $sha and (.tag_name | test("^v[0-9]+\\.[0-9]+\\.[0-9]+$")))]
      | first | .tag_name // empty
    ')
fi
if [ -z "$tag" ]; then
  echo "No release tag found for sha=$sha"
  publish_detail="no release tag found for sha=$sha"
  exit 1
fi
echo "Resolved tag=$tag for sha=$sha"
write_status

work=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/jcode-macos-release.XXXXXX")

current_step="check_existing_asset"
if gh release view "$tag" --repo "$repo" --json assets > "$work/release.json" 2>/dev/null; then
  if jq -e --arg archive "$archive" '([.assets[] | select(.size > 0) | .name] | index($archive) != null)' "$work/release.json" >/dev/null; then
    echo "Release $tag already has $archive; skipping."
    publish_state="skipped"
    publish_detail="release $tag already has $archive"
    write_status
    exit 0
  fi
else
  echo "Release $tag does not exist yet on $repo"
  publish_detail="release $tag does not exist yet on $repo"
  exit 1
fi

current_step="build_hosted_macos_binary"
write_status
JCODE_BUILD_GIT_HASH=$(git rev-parse --short "$sha")
JCODE_BUILD_GIT_DATE=$(git log -1 --format=%ci "$sha")
JCODE_BUILD_GIT_TAG=$(git describe --tags --always "$sha")
JCODE_BUILD_CHANGELOG_RAW=$(git log -700 --format='%h|%ct|%D|%s' "$sha")
export JCODE_BUILD_GIT_HASH JCODE_BUILD_GIT_DATE JCODE_BUILD_GIT_TAG JCODE_BUILD_CHANGELOG_RAW
export JCODE_BUILD_GIT_DIRTY=false
export JCODE_BUILD_SEMVER="$tag"
export JCODE_RELEASE_BUILD=1 JCODE_CI_BUILD=1
export CARGO_BUILD_RUSTC_WRAPPER=''
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3 CARGO_INCREMENTAL=0
export JUSTRUST_REMOTE_REQUIRED=1

justrust remote use hosted
justrust build --remote --platform=macos --locked --release --target "$target" -p jcode --bin jcode

current_step="verify_macho_binary"
binary="$repo_root/target/$target/release/jcode"
[ -f "$binary" ] || { echo "Missing hosted macOS binary: $binary"; exit 1; }
python3 - "$binary" <<'PY'
import pathlib, struct, sys
data = pathlib.Path(sys.argv[1]).read_bytes()[:12]
if len(data) < 8:
    raise SystemExit("error: binary too small to be Mach-O")
magic, cputype = struct.unpack_from("<II", data, 0)
if magic != 0xFEEDFACF or cputype != 0x0100000C:
    raise SystemExit(f"error: expected 64-bit ARM64 Mach-O (0xfeedfacf/0x0100000c), got 0x{magic:08x}/0x{cputype:08x}")
print("Validated Mach-O 64-bit ARM64 executable")
PY

current_step="package_archive"
stage="$work/stage"
mkdir -p "$stage"
cp "$binary" "$stage/$artifact"
chmod 755 "$stage/$artifact"
tar czf "$work/$archive" -C "$stage" "$artifact"
(cd "$work" && sha256sum "$archive" > SHA256SUMS.macos-aarch64 && sha256sum --check SHA256SUMS.macos-aarch64)

current_step="upload_release_asset"
write_status
gh release upload "$tag" "$work/$archive" "$work/SHA256SUMS.macos-aarch64" --repo "$repo" --clobber

publish_state="published"
publish_detail="uploaded $archive for $tag ($JCODE_BUILD_GIT_HASH)"
write_status
echo "=== $(date -Iseconds) macos-release uploaded $archive for $tag ($JCODE_BUILD_GIT_HASH) ==="
