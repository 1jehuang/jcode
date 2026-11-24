#!/usr/bin/env bash
# Remove stale Cargo build artifacts that Cargo never garbage-collects:
# every target/*/incremental dir, and files in target/*/deps untouched for N days.
# Cargo rebuilds anything removed. Usage: clean_stale_target.sh [--dry-run] [--days N] [target_dir]
set -euo pipefail

dry_run=0
days=14
target=""
while (( $# )); do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --days) days="$2"; shift ;;
    *) target="$1" ;;
  esac
  shift
done
target="${target:-${CARGO_TARGET_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)/target}}"
[[ -d "$target" ]] || { echo "clean_stale_target: no target dir: $target" >&2; exit 0; }

# Never delete under a running build.
if (( ! dry_run )) && pgrep -x cargo >/dev/null; then
  echo "clean_stale_target: cargo is running, skipping" >&2
  exit 0
fi

# Collect first, then delete, so find never walks a dir removed mid-scan.
stale=$(
  find "$target" -mindepth 2 -maxdepth 2 -type d -name incremental -prune
  find "$target" -mindepth 3 -maxdepth 3 -path '*/deps/*' -type f -mtime +"$days"
)
while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  if (( dry_run )); then
    echo "would remove: $path"
  else
    rm -rf -- "$path"
  fi
done <<<"$stale"
(( dry_run )) || df -h "$target" | tail -1
