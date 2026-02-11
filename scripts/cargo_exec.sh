#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

if [[ "${JCODE_REMOTE_CARGO:-0}" == "1" ]]; then
  exec "$repo_root/scripts/remote_build.sh" "$@"
fi

exec cargo "$@"
