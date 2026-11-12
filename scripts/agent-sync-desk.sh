#!/usr/bin/env bash
# agent-sync — DESK SIDE wrapper
#
# This is a thin wrapper used by the Windows scheduled-task. It runs from
# WSL bash (FedoraLinux-44) and forwards to the laptop via Tailscale SSH.
#
# The peer alias `kooshas-laptop-ts` already resolves from the WSL ssh
# config that was set up during the install (see wsl-ssh-config in scripts/).

set -u

CONFIG_PATH="${AGENT_SYNC_CONFIG:-$HOME/.config/agent-sync/config.toml}"
exec /usr/bin/env bash /home/user/.local/bin/agent-sync \
    --peer kooshas-laptop-ts --quiet "$@"
