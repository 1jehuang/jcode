#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
prompt=${1:-"Use the bash tool to run 'pwd', then use the ls tool to list the current directory, then respond with DONE."}
provider=${JCODE_PROVIDER:-auto}

if [[ ! -x "$repo_root/target/release/jcode" ]]; then
  (cd "$repo_root" && cargo build --release)
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

JCODE_HOME="$workdir" PATH="$repo_root/target/release:$PATH" \
  jcode run --no-update --trace --provider "$provider" "$prompt"
