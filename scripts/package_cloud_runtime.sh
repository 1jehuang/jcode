#!/usr/bin/env bash
# Package an explicitly built portable runtime for `jcode cloud runtime push`.
# Never builds, uploads source, or changes a running cloud daemon.
set -euo pipefail
if [[ $# != 2 ]]; then
  echo 'usage: scripts/package_cloud_runtime.sh <portable-linux-x86_64-binary> <output.tar.gz>' >&2
  echo 'build: justrust zigbuild --remote --profile selfdev -p jcode --bin jcode --target x86_64-unknown-linux-gnu.2.17 --features linux-compat-vendored-openssl' >&2
  exit 2
fi
binary=$1
output=$2
[[ -f "$binary" && -x "$binary" ]] || { echo 'missing executable runtime' >&2; exit 1; }
# Validate foreign executables structurally. Do not run an untrusted input here.
header=$(LC_ALL=C readelf -h "$binary")
[[ "$header" == *'ELF64'* && "$header" == *'Advanced Micro Devices X86-64'* ]] || {
  echo 'expected a Linux x86_64 ELF runtime, not a laptop executable' >&2; exit 1;
}
# Keep this packaging path self-contained apart from the AL2023 base libraries.
# More complex packages can use the ordinary release packaging workflow.
while IFS= read -r library; do
  case "$library" in
    libc.so.6|libm.so.6|libdl.so.2|librt.so.1|libpthread.so.0|libresolv.so.2|libgcc_s.so.1|libutil.so.1|ld-linux-x86-64.so.2) ;;
    *) echo "unsupported unbundled dependency: $library (use the portable vendored build)" >&2; exit 1 ;;
  esac
done < <(LC_ALL=C readelf -d "$binary" | awk '/NEEDED/ {gsub(/.*\[/, ""); gsub(/\].*/, ""); print}')
LC_ALL=C readelf --version-info "$binary" | awk '
  /Name: GLIBC_/ {
    for (i=1; i<=NF; i++) if ($i ~ /^GLIBC_/) {
      version=$i; sub(/^GLIBC_/, "", version); split(version, parts, ".");
      if (parts[1] + 0 > 2 || (parts[1] + 0 == 2 && parts[2] + 0 > 34)) bad=1;
      seen=1;
    }
  }
  END {exit (bad || !seen)}' || {
  echo 'runtime requires newer glibc than cloud AL2023 (2.34), or has no verifiable glibc requirements' >&2; exit 1;
}
mkdir -p -- "$(dirname -- "$output")"
stage=$(mktemp -d "$(dirname -- "$output")/.cloud-package.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
cp -- "$binary" "$stage/jcode-linux-x86_64"
chmod 755 "$stage/jcode-linux-x86_64"
tar czf "$stage/runtime.tar.gz" -C "$stage" jcode-linux-x86_64
mv -- "$stage/runtime.tar.gz" "$output"
sha256sum -- "$output"
printf 'Packaged only. Push explicitly with: jcode cloud runtime push --artifact %q\n' "$output"
