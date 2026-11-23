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
exec >>"$log_file" 2>&1

echo "=== $(date -Iseconds) linux-release start sha=$sha ==="

lock_dir="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}"
mkdir -p "$lock_dir"
exec 9>"$lock_dir/jcode-linux-release.lock"
flock 9

eligible() {
  [ "$(git config --bool jcode.linuxPrerelease || echo false)" = true ] &&
  [ "$(git symbolic-ref -q HEAD)" = refs/heads/master ] &&
  [ "$(git rev-parse HEAD)" = "$sha" ] &&
  [ -z "$(git status --porcelain=v1 --untracked-files=all --ignore-submodules=none)" ]
}

eligible || { echo "Skipping: repo not clean on master at $sha"; exit 0; }

export GH_HOST=github.com
export GH_PROMPT_DISABLED=1
repo="1jehuang/jcode"
archive="jcode-linux-x86_64.tar.gz"

remote_sha=$(gh api --hostname github.com "repos/$repo/git/ref/heads/master" --jq .object.sha)
if [ "$remote_sha" != "$sha" ]; then
  echo "Skipping: remote master ($remote_sha) != $sha"
  exit 0
fi

git fetch --tags origin --quiet || true

tag=$(git tag --points-at "$sha" --list 'v[0-9]*.[0-9]*.[0-9]*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)
if [ -z "$tag" ]; then
  tag=$(gh api "repos/$repo/releases?per_page=100" --jq \
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

work=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/jcode-linux-release.XXXXXX")
trap 'rm -rf -- "$work"' EXIT

existing=false
if gh release view "$tag" --repo "$repo" --json isDraft,isPrerelease,targetCommitish,assets,body > "$work/release.json" 2>/dev/null; then
  existing=true
  if jq -e --arg archive "$archive" '(.isDraft | not) and (.isPrerelease | not) and ([.assets[] | select(.size > 0) | .name] | index($archive) != null and index("SHA256SUMS") != null)' "$work/release.json" >/dev/null; then
    echo "Release $tag is already published with $archive and SHA256SUMS; skipping."
    exit 0
  fi
  jq -e --arg sha "$sha" '.isDraft and (.isPrerelease | not) and .targetCommitish == $sha' "$work/release.json" >/dev/null
fi

eligible || { echo "Skipping before build: repo state changed"; exit 0; }

# Build portable glibc 2.17 Linux binary on our hosted justrust service.
JCODE_BUILD_SEMVER="$tag" bash scripts/build_preview_linux_hosted.sh x86_64-unknown-linux-gnu

binary="$repo_root/target/x86_64-unknown-linux-gnu/release/jcode"
[ -x "$binary" ] || { echo "Missing hosted binary: $binary"; exit 1; }

# Verify binary version matches tag + short sha.
short_sha=$(git rev-parse --short "$sha")
"$binary" --no-update --no-selfdev --version | grep -F -- "$tag ($short_sha)"

# Optional manylinux2014 glibc 2.17 runtime smoke test when Docker is available.
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

if [ "$existing" = false ]; then
  gh release create "$tag" --repo "$repo" --target "$sha" --draft --prerelease=false --latest=false \
    --title "$title" --notes-file "$notes"
fi
gh release view "$tag" --repo "$repo" --json isDraft,isPrerelease,targetCommitish | \
  jq -e --arg sha "$sha" '.isDraft and (.isPrerelease | not) and .targetCommitish == $sha' >/dev/null
gh release upload "$tag" "$work/$archive" "$work/SHA256SUMS" --repo "$repo" --clobber
gh release edit "$tag" --repo "$repo" --title "$title" --notes-file "$notes" --draft=false --prerelease=false --latest

# Announce on Discord via lightweight workflow dispatch.
gh workflow run discord-release.yml --repo "$repo" --ref master -f "tag=$tag" || true

echo "=== $(date -Iseconds) linux-release published $tag ($short_sha) ==="
