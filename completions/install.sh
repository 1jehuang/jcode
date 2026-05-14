#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
# CarpAI Shell Completion – Installer
#
# Usage:   bash completions/install.sh [shell]
#          bash completions/install.sh          # auto-detect
#          bash completions/install.sh bash
#          bash completions/install.sh zsh
#          bash completions/install.sh fish
#          bash completions/install.sh powershell
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

CARPAI="${CARPAI:-carpai}"
SHELL="${1:-auto}"

# ── detect shell from SHELL env if "auto" ───────────────────────────
if [ "$SHELL" = "auto" ]; then
    case "${SHELL:-/bin/bash}" in
        *bash) SHELL=bash ;;
        *zsh)  SHELL=zsh  ;;
        *fish) SHELL=fish ;;
        *)     SHELL=bash ;;
    esac
fi

echo "→ Installing CarpAI completions for $SHELL ..."

case "$SHELL" in
    bash)
        DIR="${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"
        mkdir -p "$DIR"
        $CARPAI completion bash -o "$DIR/carpai"
        echo "  ✓ $DIR/carpai"
        echo "  → source $DIR/carpai  (or add to ~/.bashrc)"
        ;;
    zsh)
        DIR="${ZSH_CUSTOM:-$HOME/.zsh/completions}"
        mkdir -p "$DIR"
        $CARPAI completion zsh -o "$DIR/_carpai"
        echo "  ✓ $DIR/_carpai"
        echo "  → echo 'fpath+=($DIR)' >> ~/.zshrc && compinit"
        ;;
    fish)
        DIR="${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
        mkdir -p "$DIR"
        $CARPAI completion fish -o "$DIR/carpai.fish"
        echo "  ✓ $DIR/carpai.fish"
        echo "  → (fish auto-sources completions)"
        ;;
    powershell)
        $CARPAI completion powershell -o "$PROFILE"
        echo "  ✓ $PROFILE"
        echo "  → (PowerShell auto-loads on next launch)"
        ;;
    *)
        echo "✗ Unknown shell: $SHELL"
        echo "  Supported: bash zsh fish powershell"
        exit 1
        ;;
esac

echo "✅ CarpAI completions installed for $SHELL"
echo "   (re-open your terminal or source the file to activate)"
