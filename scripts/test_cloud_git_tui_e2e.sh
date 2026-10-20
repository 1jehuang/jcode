#!/usr/bin/env bash
# Exercise the real SSH TUI with independent local and remote Git checkouts.
# Everything, including SSH keys/configuration and both daemons, is isolated.
set -euo pipefail
: "${JCODE_BIN:?Set JCODE_BIN to the rebuilt jcode binary}"
JCODE_BIN=$(realpath "$JCODE_BIN")
W=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/cloud-git-tui.XXXXXX")
SSH_PORT=${SSH_PORT:-22928}
MODEL_PORT=${MODEL_PORT:-18928}
TMUX_SOCK=$W/tmux.sock
mkdir -p "$W"/{local-repo,remote-repo,local-home,remote-home,local-run,remote-run,ssh,bin}
cleanup() {
    tmux -S "$TMUX_SOCK" kill-server 2>/dev/null || true
    if [[ -f $W/ssh/pid ]]; then kill "$(cat "$W/ssh/pid")" 2>/dev/null || true; fi
    if [[ -n ${MODEL_PID:-} ]]; then kill "$MODEL_PID" 2>/dev/null || true; fi
    env -u JCODE_SOCKET -u JCODE_SESSION_ID JCODE_HOME="$W/local-home" JCODE_RUNTIME_DIR="$W/local-run" "$JCODE_BIN" --no-update server stop >/dev/null 2>&1 || true
    env -u JCODE_SOCKET -u JCODE_SESSION_ID JCODE_HOME="$W/remote-home" JCODE_RUNTIME_DIR="$W/remote-run" "$JCODE_BIN" --no-update server stop >/dev/null 2>&1 || true
}
trap cleanup EXIT
fail() {
    echo "FAIL: $* (artifacts: $W)" >&2
    tmux -S "$TMUX_SOCK" capture-pane -p -t git-state >&2 2>/dev/null || true
    exit 1
}
for repo in local-repo remote-repo; do
    git -C "$W/$repo" init -q -b main
    git -C "$W/$repo" config user.name 'Git widget fixture'
    git -C "$W/$repo" config user.email fixture@example.invalid
    printf 'base\n' > "$W/$repo/base.txt"
    git -C "$W/$repo" add base.txt
    git -C "$W/$repo" commit -qm base
    git -C "$W/$repo" remote add origin https://example.invalid/git-widget-fixture.git
done
printf 'local edit\n' > "$W/local-repo/local-only.txt"
printf 'remote edit\n' > "$W/remote-repo/remote-only.txt"
ssh-keygen -q -t ed25519 -N '' -f "$W/ssh/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$W/ssh/client_key"
cp "$W/ssh/client_key.pub" "$W/ssh/authorized_keys"
chmod 600 "$W/ssh/authorized_keys"
# Avoid accepting an arbitrary host key, even for a localhost fixture.
printf '[127.0.0.1]:%s %s\n' "$SSH_PORT" "$(cut -d ' ' -f 1,2 "$W/ssh/host_key.pub")" > "$W/ssh/known_hosts"
cat > "$W/ssh/config" <<EOF
Host cloud-git-fixture
    HostName 127.0.0.1
    Port $SSH_PORT
    User $(id -un)
    IdentityFile $W/ssh/client_key
    IdentitiesOnly yes
    StrictHostKeyChecking yes
    UserKnownHostsFile $W/ssh/known_hosts
EOF
cat > "$W/bin/ssh" <<EOF
#!/usr/bin/env bash
exec /usr/bin/ssh -F '$W/ssh/config' "\$@"
EOF
chmod +x "$W/bin/ssh"
cat > "$W/remote-shell" <<EOF
#!/usr/bin/env bash
export JCODE_HOME='$W/remote-home' JCODE_RUNTIME_DIR='$W/remote-run'
export JCODE_NO_TELEMETRY=1 JCODE_NO_AUTO_UPDATE=1
export JCODE_OPENAI_COMPAT_API_BASE='http://127.0.0.1:$MODEL_PORT/v1'
export OPENAI_COMPAT_API_KEY=x JCODE_OPENAI_COMPAT_DEFAULT_MODEL=fake-model
unset JCODE_SOCKET JCODE_SESSION_ID
exec bash -c "\${SSH_ORIGINAL_COMMAND:?}"
EOF
chmod +x "$W/remote-shell"
cat > "$W/ssh/sshd_config" <<EOF
Port $SSH_PORT
ListenAddress 127.0.0.1
HostKey $W/ssh/host_key
PidFile $W/ssh/pid
AuthorizedKeysFile $W/ssh/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
ForceCommand $W/remote-shell
LogLevel ERROR
EOF
/usr/bin/sshd -f "$W/ssh/sshd_config" -E "$W/ssh/log"
FAKE_LOG="$W/model.log" FAKE_LABEL=model python3 "$(dirname "$0")/fake_openai_server.py" "$MODEL_PORT" > "$W/model-output.log" 2>&1 &
MODEL_PID=$!
PATH="$W/bin:$PATH" ssh -o BatchMode=yes cloud-git-fixture 'printf fixture-ready' > "$W/probe" || fail 'SSH fixture startup'
# The local TUI stays in local-repo, while its agent and Git snapshot live in remote-repo.
printf -v launch 'cd %q && env -u JCODE_SOCKET -u JCODE_SESSION_ID PATH=%q JCODE_HOME=%q JCODE_RUNTIME_DIR=%q JCODE_NO_AUTO_UPDATE=1 JCODE_NO_TELEMETRY=1 %q --no-update --no-selfdev --ssh cloud-git-fixture --ssh-binary %q --remote-working-dir %q' \
    "$W/local-repo" "$W/bin:$PATH" "$W/local-home" "$W/local-run" "$JCODE_BIN" "$JCODE_BIN" "$W/remote-repo"
tmux -S "$TMUX_SOCK" -f /dev/null new-session -d -s git-state -x 180 -y 55 "$launch"
wait_for() {
    local text=$1
    for ((i=0; i<120; i++)); do
        tmux -S "$TMUX_SOCK" capture-pane -p -t git-state > "$W/frame.txt"
        if grep -Fq "$text" "$W/frame.txt"; then return 0; fi
        sleep 0.25
    done
    fail "missing UI text: $text"
}
wait_for remote-only.txt
wait_for local-only.txt
# Assert both were visible in the same frame, rather than on alternating pages.
grep -Fq remote-only.txt "$W/frame.txt" || fail 'remote state disappeared when local state appeared'
cp "$W/frame.txt" "$W/initial-frame.txt"
printf 'remote refresh\n' > "$W/remote-repo/remote-later.txt"
printf 'local refresh\n' > "$W/local-repo/local-later.txt"
wait_for remote-later.txt
wait_for local-later.txt
grep -Fq remote-later.txt "$W/frame.txt" || fail 'refreshed states not visible together'
cp "$W/frame.txt" "$W/refreshed-frame.txt"
printf 'PASS: independent local and remote changes visible and refreshed (artifacts: %s)\n' "$W"
