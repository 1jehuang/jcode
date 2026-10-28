#!/usr/bin/env bash
# Hosted compilation only. Packaging and native/baseline runtime acceptance belong
# to the caller. Requires a reviewed client with managed glibc217/ARM64 support.
# Published alpha.19 predates that support, despite sharing the version label
# with newer source builds. Unsupported clients must fail, never build locally.
# Usage: JCODE_BUILD_SEMVER=v1.0.0-preview.N bash scripts/build_preview_linux_hosted.sh TARGET
set -euo pipefail

fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
[[ $# == 1 ]] || fail 'usage: build_preview_linux_hosted.sh TARGET'
target=$1
case "$target" in
  x86_64-unknown-linux-gnu) toolchain=1.99.0 ;;
  aarch64-unknown-linux-gnu) toolchain=nightly-2026-10-08 ;;
  *) fail "unsupported hosted preview target: $target" ;;
esac
[[ $(uname -s) == Linux ]] || fail 'hosted Linux preview compilation requires a Linux caller'
[[ -n ${JCODE_BUILD_SEMVER:-} ]] || fail 'JCODE_BUILD_SEMVER must identify the preview version'
# Global flags can override target-only TLS or bring a development linker into
# a cross build. Refuse rather than silently dropping caller requirements.
[[ -z ${RUSTFLAGS:-} && -z ${CARGO_ENCODED_RUSTFLAGS:-} ]] || fail 'global Rust flags are unsupported, use target-specific configuration'
[[ -z ${RUSTC_BOOTSTRAP:-} ]] || fail 'RUSTC_BOOTSTRAP is not a substitute for the pinned managed compiler'
[[ -z ${CARGO_TARGET_DIR:-} && -z ${CARGO_BUILD_TARGET_DIR:-} ]] || fail 'custom target directories are unsupported by preview artifact discovery'
[[ -z ${JUSTRUST_DISABLE:-} || ${JUSTRUST_DISABLE} == 0 ]] || fail 'JUSTRUST_DISABLE conflicts with required hosted compilation'
for tool in git justrust python3 readelf; do
  command -v "$tool" >/dev/null || fail "missing required command: $tool"
done
# Match the build-metadata parser's supported preview identity before paying for
# compilation. Invalid overrides otherwise fall back to the package version.
python3 - <<'PY'
import os
import re
value = os.environ['JCODE_BUILD_SEMVER']
match = re.fullmatch(r'v?1\.0\.0-preview\.([1-9][0-9]*)', value)
if not match or len(match[1]) > 20 or int(match[1]) > 18446744073709551615:
    raise SystemExit('error: expected JCODE_BUILD_SEMVER=v1.0.0-preview.N with a positive u64 N')
PY
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

# .git is not mirrored to the tenant host. Snapshot real source metadata before
# dispatch, including dirty state, rather than embedding unknown or stale values.
JCODE_BUILD_GIT_HASH=$(git rev-parse --short HEAD)
JCODE_BUILD_GIT_DATE=$(git log -1 --format=%ci)
JCODE_BUILD_GIT_TAG=$(git describe --tags --always)
JCODE_BUILD_CHANGELOG_RAW=$(git log -700 --format='%h|%ct|%D|%s')
export JCODE_BUILD_GIT_HASH JCODE_BUILD_GIT_DATE JCODE_BUILD_GIT_TAG JCODE_BUILD_CHANGELOG_RAW
JCODE_BUILD_GIT_DIRTY=false
git_status=$(git status --porcelain)
[[ -z $git_status ]] || JCODE_BUILD_GIT_DIRTY=true
export JCODE_BUILD_GIT_DIRTY
export JCODE_RELEASE_BUILD=1 JCODE_CI_BUILD=1
export CARGO_BUILD_RUSTC_WRAPPER=''
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3 CARGO_INCREMENTAL=0
export JUSTRUST_REMOTE_REQUIRED=1 JUSTRUST_TOOLCHAIN_REQUIRED=1
export JUSTRUST_TOOLCHAIN=$toolchain
# An empty encoded global flag also overrides target flags in Cargo.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTUP_TOOLCHAIN
# Select hosted explicitly, never a legacy account-owned AWS backend. Credentials
# come from JCODE_API_KEY (CI) or the existing tenant login. No key provisioning,
# allowance changes, or billing exemptions occur here.
justrust remote use hosted
args=(--remote --locked --release --target "$target" -p jcode --bin jcode --features linux-compat-vendored-openssl)
if [[ $target == x86_64-unknown-linux-gnu ]]; then
  args=(--remote --locked --release --target "$target.2.17" -p jcode --bin jcode --features linux-compat-vendored-openssl)
  justrust zigbuild "${args[@]}"
else
  # Managed ARM setup validates/resolves Cargo target flags and injects target-
  # only emulated TLS. Pass it explicitly too, with matching nightly rust-src.
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS:+${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS} }-Ztls-model=emulated"
  justrust build "${args[@]}" -Zbuild-std=std,panic_abort
fi
artifact="target/$target/release/jcode"
[[ -x $artifact ]] || fail "hosted build did not return an executable: $artifact"

# The client validates downloaded artifacts too. Independently gate the exact
# expected file here, never glob a warm host's other release outputs. Do not run
# cross artifacts or confuse ELF/TLS inspection with native runtime acceptance.
python3 - "$artifact" "$target" <<'PY'
import os
import pathlib
import re
import struct
import subprocess
import sys

artifact, target = sys.argv[1:]
def require(condition, message):
    if not condition:
        raise SystemExit('error: ' + message)

def readelf(option):
    return subprocess.check_output(['readelf', option, artifact], text=True, env={**os.environ, 'LC_ALL': 'C'})

with pathlib.Path(artifact).open('rb') as binary:
    header = binary.read(64)
require(len(header) == 64 and header[:7] == b'\x7fELF\x02\x01\x01', 'expected little-endian ELF64 executable')
kind, machine = struct.unpack_from('<HH', header, 16)
arm = target == 'aarch64-unknown-linux-gnu'
require(kind in (2, 3) and machine == (183 if arm else 62), 'ELF executable target architecture mismatch')
program = readelf('-lW')
require(re.search(r'^\s*LOAD\s', program, re.M), 'ELF has no load segment')
loader = '/lib/ld-linux-aarch64.so.1' if arm else '/lib64/ld-linux-x86-64.so.2'
require('[Requesting program interpreter: ' + loader + ']' in program, 'unexpected GNU/Linux interpreter')
versions = readelf('-VW')
labels = re.findall(r'\bName:\s+(GLIBC_[^\s]+)', versions)
require(labels and all(re.fullmatch(r'GLIBC_\d+(?:\.\d+)+', label) for label in labels), 'missing or malformed GLIBC version requirements')
maximum = max(tuple(map(int, label[6:].split('.'))) for label in labels)
baseline = (2, 39) if arm else (2, 17)
require(maximum <= baseline, f'GLIBC requirement {maximum} exceeds {baseline}')
dynamic = readelf('-dW')
require(not re.search(r'\(NEEDED\).*\[lib(?:ssl|crypto)\.', dynamic), 'unexpected external OpenSSL dependency')
if arm:
    require(not re.search(r'^\s*TLS\s', program, re.M), 'ARM64 native PT_TLS violates emulated-TLS contract')
    symbols = readelf('-sW')
    require('__emutls_get_address' in symbols and '__emutls_v.' in symbols, 'ARM64 emulated-TLS symbols missing')
print(f'Validated hosted {target} artifact: {artifact}, GLIBC max {".".join(map(str, maximum))}')
print('Compilation and ABI checks only. Native runtime, baseline smoke, and packaging remain caller gates.')
PY
