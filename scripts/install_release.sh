#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
bin="$repo_root/target/release/jcode"

if [[ ! -x "$bin" ]]; then
  echo "Release binary not found or not executable: $bin" >&2
  echo "Run: cargo build --release" >&2
  exit 1
fi

hash=""
if command -v git >/dev/null 2>&1; then
  if git -C "$repo_root" rev-parse --git-dir >/dev/null 2>&1; then
    hash="$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || true)"
    if [[ -n "${hash}" ]] && [[ -n "$(git -C "$repo_root" status --porcelain 2>/dev/null || true)" ]]; then
      hash="${hash}-dirty"
    fi
  fi
fi

if [[ -z "$hash" ]]; then
  hash="$(date +%Y%m%d%H%M%S)"
fi

dest_dir="$HOME/.local/bin"
mkdir -p "$dest_dir"

versioned="$dest_dir/jcode-$hash"
install -m 755 "$bin" "$versioned"
ln -sfn "$versioned" "$dest_dir/jcode"

echo "Installed: $versioned"
echo "Updated symlink: $dest_dir/jcode -> $versioned"
