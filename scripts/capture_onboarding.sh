#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$repo_root/.tmp/onboarding-screenshots"}
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)

export JCODE_ONBOARDING_SCREENSHOT_DIR="$output_dir"

cd "$repo_root"
cargo test -p jcode-tui --lib onboarding_import_happy_path_images -- --ignored --nocapture

if command -v rsvg-convert >/dev/null 2>&1; then
  for svg in "$output_dir"/*.svg; do
    rsvg-convert "$svg" -o "${svg%.svg}.png"
  done
  echo "Wrote SVG and PNG onboarding screenshots to $output_dir"
else
  echo "Wrote SVG onboarding screenshots to $output_dir"
  echo "Install rsvg-convert to generate PNG copies automatically."
fi
