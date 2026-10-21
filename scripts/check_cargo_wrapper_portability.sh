#!/bin/sh
# Offline source-policy check only: never invokes Cargo, rustc, or a build.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)

# Reject wrapper settings anywhere, including quoted/dotted keys and target
# tables. Target OS selection is not a way to select the compiler host shell.
# Comments documenting an explicit opt-in are harmless. Deliberately reject
# even empty wrapper defaults: shared config must leave selection to callers.
awk '
  /^[[:space:]]*#/ { next }
  /rustc(-|_)workspace(-|_)wrapper|rustc(-|_)wrapper/ {
    print FILENAME ":" FNR ": shared Cargo config must not select a compiler wrapper" > "/dev/stderr"
    failed = 1
  }
  END { exit failed }
' "$repo_root/.cargo/config.toml"

# Retain the opt-in Unix fast path. This is a source guard, not proof that
# Cargo or a native Windows toolchain executes successfully.
grep -Fq 'export RUSTC_WRAPPER="$repo_root/scripts/rustc-parallel-frontend"' \
  "$repo_root/scripts/dev_cargo.sh"
grep -Fq 'configure_parallel_frontend "$@"' "$repo_root/scripts/dev_cargo.sh"
sh -n "$repo_root/scripts/rustc-parallel-frontend"
bash -n "$repo_root/scripts/dev_cargo.sh"
printf '%s\n' 'Cargo wrapper source policy: OK (no Cargo/compiler execution)'
