#!/usr/bin/env bash
# Publication contract for deterministic installer fixtures, not a production lock.
set -euo pipefail
source_dir='' version='' channel='' launcher=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --source) source_dir="$2"; shift 2 ;;
    --version) version="$2"; shift 2 ;;
    --channel) channel="$2"; shift 2 ;;
    --launcher) launcher="$2"; shift 2 ;;
    --no-update|publish-build) shift ;;
    *) echo "unexpected fixture publication argument: $1" >&2; exit 2 ;;
  esac
done
: "${source_dir:?}" "${version:?}" "${channel:?}" "${launcher:?}"
if [ "${TEST_UNAME_S:-${EVAL_UNAME_S:-Linux}}" = MINGW64_NT-10.0 ]; then
  builds="${JCODE_HOME:-$LOCALAPPDATA/jcode}/builds"
  binary=jcode.exe
else
  builds="${JCODE_HOME:-$HOME/.jcode}/builds"
  binary=jcode
fi
dest="$builds/versions/$version"
mkdir -p "$dest" "$builds/stable" "$(dirname "$launcher")"
cp "$source_dir/$binary" "$dest/$binary"
previous_shared=$(cat "$builds/shared-server-version" 2>/dev/null || true)
previous_stable=$(cat "$builds/stable-version" 2>/dev/null || true)
if [ ! -e "$builds/shared-server/$binary" ] \
  || { [ -n "$previous_shared" ] && [ "$previous_shared" = "$previous_stable" ]; } \
  || printf '%s' "$previous_shared" | grep -Eq '^(v?[0-9]+\.[0-9]+\.[0-9]+([+.-][[:alnum:].-]+)?|selfdev-[a-z0-9]+-[0-9a-f]{40}|main-[[:alnum:].-]+)$'; then
  mkdir -p "$builds/shared-server"
  ln -sfn "$dest/$binary" "$builds/shared-server/$binary"
  printf '%s\n' "$version" > "$builds/shared-server-version"
fi
ln -sfn "$dest/$binary" "$builds/stable/$binary"
printf '%s\n' "$version" > "$builds/stable-version"
mkdir -p "$builds/current"
ln -sfn "$dest/$binary" "$builds/current/$binary"
printf '%s\n' "$version" > "$builds/current-version"
ln -sfn "$builds/$channel/$binary" "$launcher"
