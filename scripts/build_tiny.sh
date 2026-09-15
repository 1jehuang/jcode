#!/usr/bin/env bash
# Build the minimal-footprint `jcode` binary: the `tiny` Cargo profile plus the
# reduced `minimal` feature set.
#
# What this does differently from a normal release build:
# - `JCODE_DEV_FEATURE_PROFILE=minimal` -> `--no-default-features`, dropping the
#   optional dependency stacks (local ONNX embeddings + tokenizers, the AWS
#   SDK/Bedrock provider stack, PDF extraction).
# - `--profile tiny` -> `opt-level = "z"`, fat LTO, one codegen unit, no
#   incremental, stripped symbols, `panic = "abort"`. See the `[profile.tiny]`
#   comment in Cargo.toml for the full rationale and the `panic = "abort"`
#   behavior tradeoff.
#
# This is a slow, one-off build (fat LTO over the whole graph). Use the normal
# release/selfdev profiles for iteration; use this when you want the smallest
# binary and the lowest runtime footprint.
#
# The build still goes through scripts/dev_cargo.sh so it gets the same
# memory-aware job sizing, fast-linker selection, and cargo build gate as every
# other build path in this repo.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/build_tiny.sh

Builds the minimal-footprint jcode binary with the `tiny` profile and the
`minimal` feature set, then reports the resulting size.

Environment overrides:
  JCODE_TINY_PROFILE   Cargo profile to build (default: tiny)
  JCODE_TINY_FEATURES  Feature profile name (default: minimal; "default" or
                       "full" restore the heavy optional stacks)
EOF
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
esac

if [[ "$#" -gt 0 ]]; then
  usage >&2
  exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
profile="${JCODE_TINY_PROFILE:-tiny}"
feature_profile="${JCODE_TINY_FEATURES:-minimal}"

bin="$repo_root/target/$profile/jcode"

echo "=== Minimal jcode build ==="
echo "Cargo profile:   $profile"
echo "Feature profile: $feature_profile"
echo "Output:          $bin"
echo
echo "Fat LTO with a single codegen unit is slow; this can take a long while."
echo

start=$SECONDS
(cd "$repo_root" && \
  JCODE_DEV_FEATURE_PROFILE="$feature_profile" \
  "$repo_root/scripts/dev_cargo.sh" build --profile "$profile" -p jcode --bin jcode)
elapsed=$((SECONDS - start))

if [[ ! -x "$bin" ]]; then
  echo "error: build finished but no executable found at $bin" >&2
  exit 1
fi

size_bytes=$(stat -f%z "$bin" 2>/dev/null || stat -c%s "$bin")

echo
echo "=== Result ==="
echo "Built in ${elapsed}s"
printf 'Size: %s (%s bytes)\n' "$(du -h "$bin" | awk '{print $1}')" "$size_bytes"
printf 'Version: %s\n' "$("$bin" --version 2>/dev/null || echo '(unavailable)')"

echo
echo "To apply the matching reduced runtime config:"
echo "  mkdir -p \"\${JCODE_HOME:-\$HOME/.jcode}\""
echo "  cp \"$repo_root/assets/config-templates/tiny.toml\" \"\${JCODE_HOME:-\$HOME/.jcode}/config.toml\""
echo
echo "Always launch with --no-update (the config template sets check_updates = false)."
