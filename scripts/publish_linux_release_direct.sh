#!/usr/bin/env bash
# Build and publish a portable Linux x86_64 semver release directly using the
# hosted justrust service, bypassing GitHub Actions runners.
set -euo pipefail

sha=${1:?usage: publish_linux_release_direct.sh <40-char-sha> [release_notes]}
release_notes=${2-}

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

log_dir="${JCODE_HOME:-$HOME/.jcode}/logs"
mkdir -p "$log_dir"
log_file="$log_dir/linux-release.log"
status_file="$log_dir/linux-release-status.json"
exec >>"$log_file" 2>&1

echo "=== $(date -Iseconds) linux-release start sha=$sha ==="

current_step="init"
publish_state="running"
publish_detail=""
macos_status="pending"
discord_status="pending"
tag=""
work=""

write_status() {
  local tmp="${status_file}.tmp.$$"
  if command -v jq >/dev/null 2>&1; then
    jq -n \
      --arg sha "$sha" \
      --arg tag "$tag" \
      --arg state "$publish_state" \
      --arg step "$current_step" \
      --arg macos_status "$macos_status" \
      --arg discord_status "$discord_status" \
      --arg detail "$publish_detail" \
      --arg updated_at "$(date -Iseconds)" \
      --arg log_file "$log_file" \
      '{
        sha: $sha,
        tag: (if $tag == "" then null else $tag end),
        state: $state,
        step: $step,
        macos_status: $macos_status,
        discord_status: $discord_status,
        detail: (if $detail == "" then null else $detail end),
        updated_at: $updated_at,
        log_file: $log_file
      }' >"$tmp" 2>/dev/null && mv -f "$tmp" "$status_file" || rm -f "$tmp"
  fi
}

run_with_mirror_retry() {
  local attempt=1
  local max_attempts=2
  local err_log="$work/justrust-attempt.log"
  while true; do
    if "$@" > >(tee "$err_log") 2>&1; then
      return 0
    fi
    local rc=$?
    if [ "$attempt" -lt "$max_attempts" ] && grep -q "another connection changed the source mirror" "$err_log" 2>/dev/null; then
      echo "Transient justrust source mirror collision; retrying (attempt $((attempt + 1))/$max_attempts)..."
      attempt=$((attempt + 1))
      sleep 2
      continue
    fi
    return "$rc"
  done
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
exec 9>"$lock_dir/jcode-linux-release.lock"
flock 9

current_step="check_eligibility"
eligible() {
  [ "$(git config --bool jcode.linuxPrerelease || echo false)" = true ] &&
  [ "$(git symbolic-ref -q HEAD)" = refs/heads/master ] &&
  [ "$(git rev-parse HEAD)" = "$sha" ] &&
  [ -z "$(git status --porcelain=v1 --untracked-files=all --ignore-submodules=none)" ]
}

if ! eligible; then
  echo "Skipping: repo not clean on master at $sha"
  publish_state="skipped"
  discord_status="skipped"
  publish_detail="repo not clean on master at $sha"
  write_status
  exit 0
fi

export GH_HOST=github.com
export GH_PROMPT_DISABLED=1
repo="1jehuang/jcode"
archive="jcode-linux-x86_64.tar.gz"

current_step="check_remote_master"
remote_sha=$(gh api --hostname github.com "repos/$repo/git/ref/heads/master" --jq .object.sha)
if [ "$remote_sha" != "$sha" ]; then
  echo "Skipping: remote master ($remote_sha) != $sha"
  publish_state="skipped"
  discord_status="skipped"
  publish_detail="remote master ($remote_sha) != $sha"
  write_status
  exit 0
fi

current_step="resolve_tag"
git fetch --tags origin --quiet || true

tag=$(git tag --points-at "$sha" --list 'v[0-9]*.[0-9]*.[0-9]*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)
if [ -z "$tag" ]; then
  tag=$(gh api "repos/$repo/releases?per_page=100" | jq -r \
    --arg sha "$sha" '
      [.[] | select(.draft and (.prerelease | not) and .target_commitish == $sha and (.tag_name | test("^v[0-9]+\\.[0-9]+\\.[0-9]+$")))]
      | first | .tag_name // empty
    ')
fi
if [ -z "$tag" ]; then
  base_mm=$(git show "$sha:Cargo.toml" | python3 -c 'import sys, tomllib; v = tomllib.loads(sys.stdin.read())["package"]["version"].split("."); print(f"{v[0]}.{v[1]}")')
  max_patch=$(git tag --list "v${base_mm}.*" | grep -E "^v${base_mm}\.[0-9]+$" | sed "s/^v${base_mm}\.//" | sort -n | tail -n 1 || true)
  if [ -n "$max_patch" ]; then
    tag="v${base_mm}.$((max_patch + 1))"
  else
    tag="v${base_mm}.0"
  fi
fi

echo "Resolved tag=$tag for sha=$sha"
write_status

work=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/jcode-linux-release.XXXXXX")

current_step="check_existing_release"
existing=false
if gh release view "$tag" --repo "$repo" --json isDraft,isPrerelease,targetCommitish,assets,body > "$work/release.json" 2>/dev/null; then
  existing=true
  if jq -e --arg archive "$archive" '(.isDraft | not) and (.isPrerelease | not) and ([.assets[] | select(.size > 0) | .name] | index($archive) != null and index("SHA256SUMS") != null)' "$work/release.json" >/dev/null; then
    echo "Release $tag is already published with $archive and SHA256SUMS; skipping."
    publish_state="skipped"
    discord_status="skipped"
    publish_detail="release $tag already published"
    write_status
    exit 0
  fi
  jq -e --arg sha "$sha" '.isDraft and (.isPrerelease | not) and .targetCommitish == $sha' "$work/release.json" >/dev/null
fi

if ! eligible; then
  echo "Skipping before build: repo state changed"
  publish_state="skipped"
  discord_status="skipped"
  publish_detail="repo state changed before build"
  write_status
  exit 0
fi

# Build portable glibc 2.17 Linux binary on our hosted justrust service.
current_step="build_hosted_binary"
write_status
run_with_mirror_retry env JCODE_BUILD_SEMVER="$tag" bash scripts/build_preview_linux_hosted.sh x86_64-unknown-linux-gnu

current_step="verify_binary"
binary="$repo_root/target/x86_64-unknown-linux-gnu/release/jcode"
[ -x "$binary" ] || { echo "Missing hosted binary: $binary"; exit 1; }

# Verify binary version matches tag + short sha.
short_sha=$(git rev-parse --short "$sha")
"$binary" --no-update --no-selfdev --version | grep -F -- "$tag ($short_sha)"

# Optional manylinux2014 glibc 2.17 runtime smoke test when Docker is available.
current_step="docker_smoke_test"
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  image='quay.io/pypa/manylinux2014_x86_64@sha256:2bd80fa8fca6b775a913b7e42fa410c5f2246fb6f7f8f3280b5d562e5ca5d63f'
  docker run --rm --read-only --network none --cap-drop ALL \
    --security-opt no-new-privileges --user 65534:65534 \
    --tmpfs /tmp:rw,nosuid,nodev,size=64m \
    --mount "type=bind,src=$binary,dst=/input/jcode,readonly" \
    --entrypoint /bin/bash "$image" -c '
      set -euo pipefail
      /input/jcode --no-update --no-selfdev --version
    '
fi

# Package into temp dir so we never dirty the repo working tree.
current_step="package_archive"
artifact=jcode-linux-x86_64
stage="$work/stage"
mkdir -p "$stage"
cp "$binary" "$stage/$artifact.bin"
chmod 755 "$stage/$artifact.bin"
cat > "$stage/$artifact" <<'WRAPPER'
#!/usr/bin/env sh
set -eu
self_path=$0
if command -v readlink >/dev/null 2>&1; then
  resolved=$(readlink -f -- "$0" 2>/dev/null || true)
  if [ -n "$resolved" ]; then
    self_path=$resolved
  fi
fi
case "$self_path" in
  */*) self_dir=$(CDPATH= cd -- "$(dirname -- "$self_path")" && pwd) ;;
  *) self_dir=$(pwd) ;;
esac
if [ -n "${LD_LIBRARY_PATH:-}" ]; then
  export LD_LIBRARY_PATH="$self_dir:$LD_LIBRARY_PATH"
else
  export LD_LIBRARY_PATH="$self_dir"
fi
exec "$self_dir/jcode-linux-x86_64.bin" "$@"
WRAPPER
chmod 755 "$stage/$artifact"
tar czf "$work/$archive" -C "$stage" "$artifact" "$artifact.bin"
(cd "$work" && sha256sum "$archive" > SHA256SUMS && sha256sum --check SHA256SUMS)

current_step="generate_notes"
title="jcode $tag · Linux"
notes="$work/highlights.md"
tags="$work/releases.txt"
if [ "$existing" = true ]; then
  jq -r '.body // empty' "$work/release.json" > "$notes"
fi
if [ "$existing" = false ] || ! grep -Fxq '## Highlights' "$notes"; then
  gh api --paginate "repos/$repo/releases?per_page=100" \
    --jq '.[] | select(.draft == false) | [.published_at, .tag_name] | @tsv' \
    | LC_ALL=C sort -r | cut -f2 > "$tags"
  SHA="$sha" RELEASE_NOTES="$release_notes" python3 scripts/selfdev_release_highlights.py "$tags" > "$notes"
fi

current_step="publish_github_release"
write_status
if [ "$existing" = false ]; then
  gh release create "$tag" --repo "$repo" --target "$sha" --draft --prerelease=false --latest=false \
    --title "$title" --notes-file "$notes"
fi
gh release view "$tag" --repo "$repo" --json isDraft,isPrerelease,targetCommitish | \
  jq -e --arg sha "$sha" '.isDraft and (.isPrerelease | not) and .targetCommitish == $sha' >/dev/null
gh release upload "$tag" "$work/$archive" "$work/SHA256SUMS" --repo "$repo" --clobber

# Build and upload macOS arm64 asset on hosted justrust when enabled (default: on).
current_step="build_hosted_macos"
write_status
if [ "${JCODE_SKIP_MACOS_RELEASE:-0}" != "1" ] && [ -x scripts/publish_macos_release_direct.sh ]; then
  if run_with_mirror_retry bash scripts/publish_macos_release_direct.sh "$sha" "$tag"; then
    macos_status="published"
    title="jcode $tag · Linux + macOS"
  else
    echo "Warning: hosted macOS arm64 release build failed for $tag; continuing with Linux release"
    macos_status="failed"
  fi
else
  macos_status="skipped"
fi
write_status

gh release edit "$tag" --repo "$repo" --title "$title" --notes-file "$notes" --draft=false --prerelease=false --latest

# Announce on Discord directly when a local webhook is available, else fall back to workflow dispatch.
current_step="dispatch_discord"
write_status
discord_webhook="${DISCORD_RELEASE_WEBHOOK:-}"
if [ -z "$discord_webhook" ] && [ -f "${JCODE_HOME:-$HOME/.jcode}/discord-release-webhook" ]; then
  discord_webhook=$(tr -d '\r\n' < "${JCODE_HOME:-$HOME/.jcode}/discord-release-webhook")
fi
if [ -n "$discord_webhook" ] && [ -f scripts/post_discord_release.py ]; then
  gh_token="${GH_TOKEN:-$(gh auth token 2>/dev/null || true)}"
  if ! GITHUB_REPOSITORY="$repo" GH_TOKEN="$gh_token" DISCORD_RELEASE_WEBHOOK="$discord_webhook" python3 scripts/post_discord_release.py --tag "$tag"; then
    echo "Failed to post Discord announcement directly for $tag"
    discord_status="failed"
    publish_state="failed"
    publish_detail="direct post_discord_release.py failed for $tag"
    write_status
    exit 1
  fi
  discord_status="posted"
else
  if ! gh workflow run discord-release.yml --repo "$repo" --ref master -f "tag=$tag"; then
    echo "Failed to dispatch discord-release.yml for $tag"
    discord_status="failed"
    publish_state="failed"
    publish_detail="gh workflow run discord-release.yml failed for $tag"
    write_status
    exit 1
  fi
  discord_status="dispatched"
fi

publish_state="published"
publish_detail="published $tag ($short_sha) and announced on Discord ($discord_status)"
write_status

echo "=== $(date -Iseconds) linux-release published $tag ($short_sha) ==="
