#!/usr/bin/env bash
set -euo pipefail

# Guardrail for GUI worktree branches: keep staged changes scoped to GUI files.
# Usage:
#   scripts/check_gui_scope.sh            # check staged files
#   scripts/check_gui_scope.sh --all      # check all working tree changes

branch="$(git symbolic-ref --quiet --short HEAD || true)"
if [[ -z "$branch" ]]; then
  exit 0
fi

if [[ "$branch" != *"dioxus-gui"* ]]; then
  exit 0
fi

mode="${1:---staged}"
if [[ "$mode" == "--all" ]]; then
  changed_files="$(git diff --name-only HEAD)"
elif [[ "$mode" == "--staged" ]]; then
  changed_files="$(git diff --name-only --cached)"
else
  echo "Unknown mode: $mode" >&2
  echo "Usage: scripts/check_gui_scope.sh [--staged|--all]" >&2
  exit 2
fi

if [[ -z "${changed_files}" ]]; then
  exit 0
fi

violations=()
while IFS= read -r file; do
  [[ -z "$file" ]] && continue
  case "$file" in
    jcode-gui/*) ;;
    Cargo.toml) ;;
    .github/workflows/remote-build.yml) ;;
    scripts/check_gui_scope.sh) ;;
    .githooks/pre-commit) ;;
    *) violations+=("$file") ;;
  esac
done <<< "$changed_files"

if [[ "${#violations[@]}" -gt 0 ]]; then
  echo "Blocked: non-GUI files staged on branch '$branch'." >&2
  echo "Allowed paths: jcode-gui/*, Cargo.toml, .github/workflows/remote-build.yml, scripts/check_gui_scope.sh, .githooks/pre-commit" >&2
  echo "Violations:" >&2
  for file in "${violations[@]}"; do
    echo "  - $file" >&2
  done
  echo "If intentional, move changes to a non-GUI branch/worktree first." >&2
  exit 1
fi

exit 0
