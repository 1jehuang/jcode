#!/usr/bin/env bash
# Package a hosted portable build only after execution in the glibc 2.17 baseline.
set -euo pipefail
[[ $# == 0 ]] || { echo 'usage: package_preview_linux_hosted.sh' >&2; exit 1; }
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
artifact=jcode-linux-x86_64
binary="$repo_root/target/x86_64-unknown-linux-gnu/release/jcode"
image='quay.io/pypa/manylinux2014_x86_64@sha256:2bd80fa8fca6b775a913b7e42fa410c5f2246fb6f7f8f3280b5d562e5ca5d63f'
[[ -x $binary ]] || { echo "missing hosted executable: $binary" >&2; exit 1; }
# Runtime acceptance is separate from the hosted client's structured ELF checks.
# The baseline container cannot write source, access credentials or use networking.
docker run --rm --read-only --network none --cap-drop ALL \
  --security-opt no-new-privileges --user 65534:65534 \
  --tmpfs /tmp:rw,nosuid,nodev,size=64m \
  --mount "type=bind,src=$binary,dst=/input/jcode,readonly" \
  --entrypoint /bin/bash "$image" -c '
    set -euo pipefail
    /input/jcode --no-update --no-selfdev --version
    for command in receive credentials router-credentials cancel-return; do
      /input/jcode --no-update --no-selfdev cloud "$command" --help >/dev/null
    done
  '
mkdir -p dist
stage=$(mktemp -d "$repo_root/dist/.hosted-linux-package.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
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
tar czf "$stage/$artifact.tar.gz" -C "$stage" "$artifact" "$artifact.bin"
# Never publish an archive left over from a failed smoke or packaging attempt.
for name in "$artifact" "$artifact.bin" "$artifact.tar.gz"; do
  mv -f -- "$stage/$name" "dist/$name"
done
printf 'Packaged hosted portable Linux build after baseline runtime acceptance\n'
