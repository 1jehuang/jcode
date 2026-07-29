#!/usr/bin/env bash
# Shared helper that ensures a directory is on PATH for bash, zsh, and fish.
#
# Usage:
#   source "$(dirname "$0")/lib/configure_path.sh"
#   jcode_configure_path "/path/to/install/dir"
#
# This is intentionally POSIX-friendly and side-effect free until you call
# jcode_configure_path. It is kept in sync with the inline copy in install.sh,
# which must stay self-contained because it is run via `curl ... | bash`.

# Configure PATH for bash, zsh and fish.
#   jcode_configure_path <install-dir> [report-fn]
# report-fn (optional) is called with a human-readable summary string.
jcode_configure_path() {
  _jcp_install_dir="$1"
  _jcp_report="${2:-}"
  _jcp_path_line="export PATH=\"$_jcp_install_dir:\$PATH\""
  _jcp_added=""

  _jcp_have() { command -v "$1" >/dev/null 2>&1; }

  # Append the POSIX (bash/zsh/sh) PATH line to an rc file, idempotently.
  #   _jcp_posix_rc <rc-file> <create:yes|no>
  # With create=yes the file (and parent dir) is created if missing; with
  # create=no we only touch files that already exist, so we never change how a
  # login shell resolves its startup files (e.g. creating ~/.bash_profile would
  # stop bash from reading ~/.profile).
  _jcp_posix_rc() {
    _rc="$1"; _create="$2"
    if [ ! -f "$_rc" ]; then
      [ "$_create" = "yes" ] || return 0
      mkdir -p "$(dirname "$_rc")"
    fi
    if ! grep -qF "$_jcp_install_dir" "$_rc" 2>/dev/null; then
      printf '\n# Added by jcode installer\n%s\n' "$_jcp_path_line" >> "$_rc"
      _jcp_added="$_jcp_added $_rc"
    fi
  }

  # fish uses its own syntax and does not read POSIX rc files.
  _jcp_fish_rc() {
    _create="$1"
    _rc="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
    if [ ! -f "$_rc" ]; then
      [ "$_create" = "yes" ] || return 0
      mkdir -p "$(dirname "$_rc")"
    fi
    if ! grep -qF "$_jcp_install_dir" "$_rc" 2>/dev/null; then
      {
        printf '\n# Added by jcode installer\n'
        printf 'if not contains "%s" $PATH\n' "$_jcp_install_dir"
        printf '    set -gx PATH "%s" $PATH\n' "$_jcp_install_dir"
        printf 'end\n'
      } >> "$_rc"
      _jcp_added="$_jcp_added $_rc"
    fi
  }

  # zsh: ~/.zshenv is read for every zsh invocation (login, interactive and
  # scripts), so it is the most reliable single place to export PATH.
  if _jcp_have zsh || [ "$(uname -s)" = "Darwin" ] || [ -f "$HOME/.zshenv" ] || [ -f "$HOME/.zshrc" ]; then
    _jcp_posix_rc "$HOME/.zshenv" yes
  fi

  # bash: ~/.bashrc for interactive shells, ~/.profile for login shells.
  if _jcp_have bash || [ -f "$HOME/.bashrc" ] || [ -f "$HOME/.bash_profile" ]; then
    _jcp_posix_rc "$HOME/.bashrc" yes
  fi
  _jcp_posix_rc "$HOME/.profile" yes

  # fish: only set it up when fish is installed or already configured.
  if _jcp_have fish || [ -f "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ]; then
    _jcp_fish_rc yes
  fi

  # Also patch other common startup files when they already exist.
  for _rc in "$HOME/.zshrc" "$HOME/.zprofile" "$HOME/.bash_profile"; do
    _jcp_posix_rc "$_rc" no
  done

  if [ -n "$_jcp_added" ]; then
    if [ -n "$_jcp_report" ] && command -v "$_jcp_report" >/dev/null 2>&1; then
      "$_jcp_report" "Added $_jcp_install_dir to PATH in:$_jcp_added"
    else
      printf 'Added %s to PATH in:%s\n' "$_jcp_install_dir" "$_jcp_added"
    fi
  fi
}
