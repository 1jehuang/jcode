#!/usr/bin/env bash
# Regenerate SHA256SUMS for a release from the assets currently attached to it.
#
# Releases publish asynchronously: the fast platforms go public first and slow
# platforms attach their assets later. Every job that uploads an asset runs
# this afterwards so SHA256SUMS always covers exactly the attached binaries.
# Concurrent runs converge: each pass rereads the asset list and repeats until
# the uploaded manifest matches what is attached.
#
# Uses GitHub's per-asset sha256 digest when available and falls back to
# downloading the asset. Requires gh and sha256sum (Git Bash on Windows).
set -euo pipefail

tag="${1:?usage: $0 <tag>}"
repo="${GH_REPO:-${GITHUB_REPOSITORY:-1jehuang/jcode}}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

release_id() {
  # releases/tags/<tag> 404s for drafts, so search the authenticated list.
  gh api "repos/$repo/releases?per_page=100" --paginate \
    --jq ".[] | select(.tag_name == \"$tag\") | .id" | head -n1
}

id="$(release_id)"
[ -n "$id" ] || { echo "No release found for $tag" >&2; exit 1; }

for attempt in 1 2 3 4 5; do
  # name<TAB>digest<TAB>download-url, for binaries plus provenance.
  gh api "repos/$repo/releases/$id/assets?per_page=100" --paginate \
    --jq '.[] | select(.state == "uploaded") | select((.name | endswith(".tar.gz")) or (.name | endswith(".exe")) or .name == "release-provenance.json") | [.name, (.digest // ""), .url] | @tsv' \
    | sort > "$work/assets.tsv"
  [ -s "$work/assets.tsv" ] || { echo "No assets attached to $tag" >&2; exit 1; }

  : > "$work/SHA256SUMS"
  while IFS=$'\t' read -r name digest url; do
    case "$digest" in
      sha256:*) sum="${digest#sha256:}" ;;
      *)
        gh api -H "Accept: application/octet-stream" "$url" > "$work/blob"
        sum="$(sha256sum "$work/blob" | awk '{print $1}')"
        ;;
    esac
    printf '%s  %s\n' "$sum" "$name" >> "$work/SHA256SUMS"
  done < "$work/assets.tsv"

  (cd "$work" && gh release upload "$tag" SHA256SUMS --repo "$repo" --clobber)

  # Another job may have attached an asset while we hashed. Repeat until the
  # attached set equals the set we just covered.
  gh api "repos/$repo/releases/$id/assets?per_page=100" --paginate \
    --jq '.[] | select(.state == "uploaded") | select((.name | endswith(".tar.gz")) or (.name | endswith(".exe")) or .name == "release-provenance.json") | .name' \
    | sort > "$work/after.txt"
  cut -f1 "$work/assets.tsv" > "$work/before.txt"
  if cmp -s "$work/before.txt" "$work/after.txt"; then
    echo "SHA256SUMS for $tag covers $(wc -l < "$work/before.txt" | tr -d ' ') assets"
    cat "$work/SHA256SUMS"
    exit 0
  fi
  echo "Asset set changed during refresh (attempt $attempt); retrying" >&2
  sleep $((attempt * 3))
done
echo "SHA256SUMS did not converge for $tag" >&2
exit 1
