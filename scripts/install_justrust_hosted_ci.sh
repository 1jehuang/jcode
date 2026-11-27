#!/usr/bin/env bash
# Install the latest published Linux justrust client binary with SHA256 verification.
set -euo pipefail
[[ $# == 0 ]] || { echo 'usage: install_justrust_hosted_ci.sh' >&2; exit 1; }
case "$(uname -m)" in
  x86_64|amd64) target=x86_64-unknown-linux-musl ;;
  aarch64|arm64) target=aarch64-unknown-linux-musl ;;
  *) echo "unsupported Linux architecture: $(uname -m)" >&2; exit 1 ;;
esac
work=$(mktemp -d "${RUNNER_TEMP:-${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}}/justrust-hosted-client.XXXXXX")
mkdir -p "$work/install/bin"
archive="justrust-${target}.tar.gz"
base="https://github.com/1jehuang/justrust/releases/latest/download"
curl -fsSL --retry 3 --connect-timeout 15 -o "$work/$archive" "$base/$archive"
curl -fsSL --retry 3 --connect-timeout 15 -o "$work/$archive.sha256" "$base/$archive.sha256"
(cd "$work" && sha256sum --check "$archive.sha256")
tar -xzf "$work/$archive" -C "$work/install/bin" --strip-components=1 "justrust-${target}/justrust"
chmod +x "$work/install/bin/justrust"
"$work/install/bin/justrust" --version
if [[ -n ${GITHUB_PATH:-} ]]; then
  printf '%s\n' "$work/install/bin" >> "$GITHUB_PATH"
fi
printf 'Hosted justrust client binary: %s\n' "$work/install/bin/justrust"
