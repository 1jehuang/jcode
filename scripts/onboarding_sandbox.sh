#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
command=${1:-help}
if [[ $# -gt 0 ]]; then
  shift
fi

sandbox_name=${JCODE_ONBOARDING_SANDBOX:-default}
sandbox_root_default="$repo_root/.tmp/onboarding/$sandbox_name"
sandbox_root=${JCODE_ONBOARDING_DIR:-$sandbox_root_default}
jcode_home="$sandbox_root/home"
runtime_dir="$sandbox_root/runtime"
mobile_socket="$runtime_dir/jcode-mobile-sim.sock"

ensure_dirs() {
  mkdir -p "$jcode_home" "$runtime_dir"
}

run_in_sandbox() {
  ensure_dirs
  (
    cd "$repo_root"
    # Strip any inherited self-dev markers so the sandbox behaves like a real
    # first-run install. `--no-selfdev` only prevents *setting*
    # JCODE_CLIENT_SELFDEV_MODE; it cannot unset one inherited from a parent
    # self-dev shell, which would otherwise force every sandbox session canary
    # (suppressing the new-session suggestion cards we are trying to verify).
    env -u JCODE_CLIENT_SELFDEV_MODE -u JCODE_SELFDEV -u JCODE_CANARY \
      JCODE_HOME="$jcode_home" \
      JCODE_RUNTIME_DIR="$runtime_dir" \
      "$@"
  )
}


print_usage() {
  cat <<EOF
Usage: $(basename "$0") <command> [args...]

Commands:
  env                    Print the sandbox environment exports
  status                 Show sandbox paths and current contents
  reset                  Delete the sandbox entirely
  shell                  Open a clean shell with sandbox env vars set
  jcode [args...]        Run jcode inside the sandbox
  auth-status            Run 'jcode auth status' inside the sandbox
  fresh [args...]        Reset sandbox, then launch jcode with args
  seed-real-logins [--with-transcripts|--transcripts-only]
                         Copy your REAL external logins (Codex/Claude/Gemini/
                         Copilot/Cursor/OpenCode/pi) into the sandbox so the
                         onboarding import step can import them. Add
                         --with-transcripts to also copy Codex/Claude
                         transcripts (so "continue where you left off" has data).
                         Originals are never modified.
  fresh-real [--with-transcripts]
                         Reset sandbox, seed your real logins, then launch jcode
  login <provider> ...   Run 'jcode --provider <provider> login ...' in sandbox
  fixture-list           List saved local auth fixtures
  fixture-save <name>    Save current sandbox auth state as a local fixture
  fixture-load <name>    Load a saved auth fixture into this sandbox
  fixture-run <name> -- [args...]
                         Load a fixture, then run jcode with args
  mobile-start [scenario]
                         Start jcode-mobile-sim in background (default: onboarding)
  mobile-serve [scenario]
                         Run jcode-mobile-sim in foreground (default: onboarding)
  mobile-status          Show mobile simulator status
  mobile-state           Show full mobile simulator state
  mobile-reset           Reset the mobile simulator back to its initial scenario
  mobile-log             Show mobile simulator transition log
  help                   Show this help

Environment overrides:
  JCODE_ONBOARDING_SANDBOX   Sandbox name (default: default)
  JCODE_ONBOARDING_DIR       Explicit sandbox directory
  JCODE_AUTH_FIXTURE_DIR     Fixture store (default: .tmp/auth-fixtures)

Examples:
  $(basename "$0") fresh
  $(basename "$0") login openai
  $(basename "$0") fixture-save normal-openai
  $(basename "$0") fixture-load normal-openai
  $(basename "$0") auth-status
  $(basename "$0") mobile-start onboarding
  $(basename "$0") mobile-status
EOF
}

print_env() {
  ensure_dirs
  cat <<EOF
export JCODE_HOME="$jcode_home"
export JCODE_RUNTIME_DIR="$runtime_dir"
EOF
}

status() {
  ensure_dirs
  echo "Sandbox name: $sandbox_name"
  echo "Sandbox root: $sandbox_root"
  echo "JCODE_HOME:   $jcode_home"
  echo "RUNTIME_DIR:  $runtime_dir"
  echo

  if [[ -d "$jcode_home" ]]; then
    echo "Home contents:"
    find "$jcode_home" -maxdepth 3 \( -type f -o -type d \) | sed "s#^$sandbox_root#.#" | sort
  fi
  echo

  if [[ -S "$mobile_socket" ]]; then
    echo "Mobile simulator socket: $mobile_socket"
  else
    echo "Mobile simulator socket: not running"
  fi
}

reset() {
  rm -rf "$sandbox_root"
  echo "Removed onboarding sandbox: $sandbox_root"
}

open_shell() {
  ensure_dirs
  echo "Opening sandbox shell"
  echo "  JCODE_HOME=$jcode_home"
  echo "  JCODE_RUNTIME_DIR=$runtime_dir"
  env JCODE_HOME="$jcode_home" JCODE_RUNTIME_DIR="$runtime_dir" bash --noprofile --norc
}

run_jcode() {
  # The sandbox should behave like a real standalone install, not a self-dev
  # client. Because we launch from inside the repo, jcode would otherwise
  # auto-detect the repository and join the shared self-dev server (remote
  # mode), which both breaks isolation and skips local-only first-run behavior
  # like the new-session model validation. `--no-selfdev` keeps it standalone,
  # spawning its own server under the sandbox's JCODE_RUNTIME_DIR. Set
  # JCODE_SANDBOX_SELFDEV=1 to opt back into the shared-server behavior.
  local prefix=()
  if [[ "${JCODE_SANDBOX_SELFDEV:-0}" != "1" ]]; then
    prefix=(--no-selfdev)
  fi
  # Allow pointing the sandbox at an already-built binary (e.g. the selfdev
  # profile output) without rebuilding the debug binary. Falls back to the
  # debug binary, then to `cargo run`.
  if [[ -n "${JCODE_SANDBOX_BIN:-}" ]]; then
    if [[ -x "$JCODE_SANDBOX_BIN" ]]; then
      run_in_sandbox "$JCODE_SANDBOX_BIN" "${prefix[@]}" "$@"
      return
    fi
    echo "JCODE_SANDBOX_BIN=$JCODE_SANDBOX_BIN is not executable" >&2
    return 1
  fi
  local binary_path="$repo_root/target/debug/jcode"
  if [[ -x "$binary_path" ]]; then
    run_in_sandbox "$binary_path" "${prefix[@]}" "$@"
  else
    run_in_sandbox cargo run --bin jcode -- "${prefix[@]}" "$@"
  fi
}

run_mobile_sim() {
  local binary_path="$repo_root/target/debug/jcode-mobile-sim"
  if [[ -x "$binary_path" ]]; then
    run_in_sandbox "$binary_path" "$@"
  else
    run_in_sandbox cargo run -p jcode-mobile-sim -- "$@"
  fi
}

run_auth_fixture() {
  JCODE_ONBOARDING_SANDBOX="$sandbox_name" \
    JCODE_ONBOARDING_DIR="$sandbox_root" \
    "$repo_root/scripts/auth_fixture.sh" "$@"
}

# Copy one real file from $HOME into the sandbox's external/ tree, preserving its
# relative path. jcode resolves every external credential/transcript lookup to
# $JCODE_HOME/external/<same-relative-path-as-$HOME> when JCODE_HOME is set, so
# seeding here makes your real logins/transcripts visible to the onboarding
# import + continue steps. Copies (never symlinks: jcode rejects symlinked auth
# files) and never touches the originals.
seed_one_file() {
  local rel=$1
  local src="$HOME/$rel"
  local dst="$jcode_home/external/$rel"
  if [[ ! -e "$src" ]]; then
    return 1
  fi
  mkdir -p "$(dirname "$dst")"
  cp -a "$src" "$dst"
  chmod -R go-rwx "$jcode_home/external" 2>/dev/null || true
  return 0
}

# Copy a real directory subtree (e.g. transcript stores) into external/.
seed_one_dir() {
  local rel=$1
  local src="$HOME/$rel"
  local dst="$jcode_home/external/$rel"
  if [[ ! -d "$src" ]]; then
    return 1
  fi
  mkdir -p "$dst"
  cp -a "$src/." "$dst/"
  chmod -R go-rwx "$jcode_home/external" 2>/dev/null || true
  return 0
}

# Seed the sandbox with copies of your real external logins (and, with
# --with-transcripts, your Codex/Claude transcripts) so onboarding's import and
# "continue where you left off" steps act on real data.
seed_real_logins() {
  ensure_dirs

  local with_transcripts=0
  for arg in "$@"; do
    case "$arg" in
      --with-transcripts) with_transcripts=1 ;;
      --transcripts-only) with_transcripts=2 ;;
      *) echo "Unknown seed-real-logins option: $arg" >&2; return 2 ;;
    esac
  done

  # Auth/credential files the external-import detectors read, each relative to
  # $HOME (mirrors crate::storage::user_home_path and the per-provider paths).
  local auth_files=(
    ".codex/auth.json"
    ".claude/.credentials.json"
    ".claude.json"
    ".local/share/opencode/auth.json"
    ".pi/agent/auth.json"
    ".gemini/oauth_creds.json"
    ".config/github-copilot/hosts.json"
    ".config/github-copilot/apps.json"
    ".cursor/auth.json"
    ".config/cursor/auth.json"
    ".config/Cursor/User/globalStorage/state.vscdb"
    ".config/cursor/User/globalStorage/state.vscdb"
  )

  # Transcript stores the "continue where you left off" picker reads.
  local transcript_dirs=(
    ".codex/sessions"
    ".claude/projects"
  )

  local seeded=()
  local skipped=()

  if [[ $with_transcripts -ne 2 ]]; then
    for rel in "${auth_files[@]}"; do
      if seed_one_file "$rel"; then
        seeded+=("$rel")
      else
        skipped+=("$rel")
      fi
    done
  fi

  if [[ $with_transcripts -ge 1 ]]; then
    for rel in "${transcript_dirs[@]}"; do
      if seed_one_dir "$rel"; then
        seeded+=("$rel/ (transcripts)")
      else
        skipped+=("$rel/ (transcripts)")
      fi
    done
  fi

  echo "Seeded real logins into sandbox external dir:"
  echo "  $jcode_home/external"
  echo
  if [[ ${#seeded[@]} -gt 0 ]]; then
    echo "Copied:"
    for rel in "${seeded[@]}"; do
      echo "  + $rel"
    done
  else
    echo "Copied nothing (no matching real files found under \$HOME)."
  fi
  if [[ ${#skipped[@]} -gt 0 ]]; then
    echo
    echo "Not present (skipped):"
    for rel in "${skipped[@]}"; do
      echo "  - $rel"
    done
  fi
  echo
  echo "These are copies; your real \$HOME files are untouched."
  echo "Onboarding will now offer to import them. Start it with:"
  echo "  $(basename "$0") jcode"
}

scenario_arg() {
  if [[ $# -gt 0 ]]; then
    printf '%s' "$1"
  else
    printf 'onboarding'
  fi
}

case "$command" in
  env)
    print_env
    ;;
  status)
    status
    ;;
  reset)
    reset
    ;;
  shell)
    open_shell
    ;;
  jcode)
    run_jcode "$@"
    ;;
  auth-status)
    run_jcode auth status
    ;;
  fresh)
    reset
    run_jcode "$@"
    ;;
  seed-real-logins)
    seed_real_logins "$@"
    ;;
  fresh-real)
    reset
    seed_real_logins "$@"
    echo
    echo "Launching sandbox jcode with your real logins available to import..."
    run_jcode
    ;;
  login)
    if [[ $# -lt 1 ]]; then
      echo "login requires a provider, for example: $(basename "$0") login openai" >&2
      exit 1
    fi
    provider=$1
    shift
    run_jcode --provider "$provider" login "$@"
    ;;
  fixture-list)
    run_auth_fixture list
    ;;
  fixture-save)
    run_auth_fixture save "$@"
    ;;
  fixture-load)
    run_auth_fixture load "$@"
    ;;
  fixture-run)
    run_auth_fixture run "$@"
    ;;
  mobile-start)
    scenario=$(scenario_arg "$@")
    run_mobile_sim start --scenario "$scenario"
    ;;
  mobile-serve)
    scenario=$(scenario_arg "$@")
    run_mobile_sim serve --scenario "$scenario"
    ;;
  mobile-status)
    run_mobile_sim status
    ;;
  mobile-state)
    run_mobile_sim state
    ;;
  mobile-reset)
    run_mobile_sim reset
    ;;
  mobile-log)
    run_mobile_sim log
    ;;
  help|-h|--help)
    print_usage
    ;;
  *)
    echo "Unknown command: $command" >&2
    echo >&2
    print_usage >&2
    exit 1
    ;;
esac
