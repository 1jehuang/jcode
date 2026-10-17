#!/usr/bin/env bash
# Source-only helpers for local KCode install paths. No filesystem mutations.
require_kcode_owned_path() {
  local candidate="$1" part current="" lowered
  case "$candidate" in
    /*) ;;
    *) echo 'KCode product paths must be absolute.' >&2; return 1 ;;
  esac
  local parts=()
  IFS='/' read -r -a parts <<< "$candidate"
  for part in "${parts[@]}"; do
    [[ -z "$part" ]] && continue
    lowered="$(printf '%s' "$part" | LC_ALL=C tr '[:upper:]' '[:lower:]')"
    case "$lowered" in
      ..|.jcode*|jcode|jcode.*|jcode-*)
        echo 'KCode product paths cannot use jcode storage or launchers.' >&2
        return 1 ;;
    esac
    current="$current/$part"
    if [[ -L "$current" ]]; then
      echo 'KCode product paths cannot traverse symlinks.' >&2
      return 1
    fi
  done
}
