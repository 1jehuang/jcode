#!/usr/bin/env bash
# Published alpha.19 predates managed hosted Linux release targets. Bootstrap a
# reviewed immutable source revision rather than trusting the version string.
set -euo pipefail
[[ $# == 0 ]] || { echo 'usage: install_justrust_hosted_ci.sh' >&2; exit 1; }
readonly revision=5c8c6e653d1874923cf94b93dd96aa3d312343f0
work=$(mktemp -d "${RUNNER_TEMP:-${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}}/justrust-hosted-client.XXXXXX")
# Keep the installed binary until the job ends. Source excludes the vendored
# compiler, server and any user configuration. No credentials are needed here.
git init -q "$work/source"
git -C "$work/source" remote add origin https://github.com/1jehuang/justrust.git
git -C "$work/source" sparse-checkout set --no-cone \
  '/Cargo.toml' '/Cargo.lock' '/src/' '/scripts/sign-release.py' '/docs/AGENTS-snippet.md' '/README.md' '/LICENSE*'
git -C "$work/source" fetch --quiet --depth=1 --filter=blob:none origin "$revision"
git -C "$work/source" checkout --quiet --detach FETCH_HEAD
[[ $(git -C "$work/source" rev-parse HEAD) == "$revision" ]] || {
  echo 'hosted justrust source revision mismatch' >&2
  exit 1
}
# This small client bootstrap is local. Application release compilation is
# required to be hosted by build_preview_linux_hosted.sh, with no fallback.
JUSTRUST_RELEASE_BUILD=0 CARGO_TARGET_DIR="$work/target" \
  cargo +1.99.0 install --path "$work/source" --locked --root "$work/install"
"$work/install/bin/justrust" --version
# Capability preflight also catches accidental use of an older published client.
"$work/install/bin/justrust" zigbuild --help >/dev/null
if [[ -n ${GITHUB_PATH:-} ]]; then
  printf '%s\n' "$work/install/bin" >> "$GITHUB_PATH"
fi
printf 'Hosted justrust client source: %s\n' "$revision"
printf 'Hosted justrust client binary: %s\n' "$work/install/bin/justrust"
