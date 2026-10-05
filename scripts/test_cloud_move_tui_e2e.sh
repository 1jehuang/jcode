#!/usr/bin/env bash
# Interactive TUI e2e for /cloud and /local.
#
# - "cloud host": a user-level sshd on 127.0.0.1:$SSH_PORT whose login shell
#   environment forces JCODE_HOME/RUNTIME into $W/cloud-home (ForceCommand
#   wrapper), so it never touches the user's real ~/.jcode or daemon.
# - "laptop": a private tmux server running the jcode TUI with
#   JCODE_HOME=$W/local-home and its own runtime dir / daemon socket.
# Same repo path on both (same box), so the cloud side uses a *separate*
# repo directory via a bind-mount namespace in the ForceCommand wrapper.
set -euo pipefail
JCODE_BIN=${JCODE_BIN:?}
W=$(mktemp -d "${JCODE_SCRATCH_DIR:-/tmp}/cloud-tui-e2e.XXXXXX")
REPO=/tmp/jcode-cloud-tui-e2e-repo
SSH_PORT=${SSH_PORT:-22917}
MODEL_PORT=${MODEL_PORT:-18912}
TMUX_SOCK=$W/tmux.sock
mkdir -p "$W/local-home" "$W/local-run" "$W/cloud-home" "$W/cloud-run" "$W/cloud-repo" "$W/ssh"
if [ -e "$REPO" ]; then rm -rf -- "/tmp/jcode-cloud-tui-e2e-repo"; fi

pass() { printf '  PASS %s\n' "$*"; }
fail() { printf '  FAIL %s\n' "$*"; tmux -S "$TMUX_SOCK" capture-pane -p -t e2e 2>/dev/null | tail -40; exit 1; }
cleanup() {
  tmux -S "$TMUX_SOCK" kill-server 2>/dev/null || true
  [ -f "$W/sshd.pid" ] && kill "$(cat "$W/sshd.pid")" 2>/dev/null || true
  [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null || true
  # stop isolated daemons
  JCODE_HOME=$W/local-home JCODE_RUNTIME_DIR=$W/local-run "$JCODE_BIN" --no-update server stop >/dev/null 2>&1 || true
  JCODE_HOME=$W/cloud-home JCODE_RUNTIME_DIR=$W/cloud-run "$JCODE_BIN" --no-update server stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

FAKE_LOG=$W/model.log FAKE_LABEL=model python3 "$(dirname "$0")/fake_openai_server.py" "$MODEL_PORT" &
SRV=$!
MODEL_ENV="JCODE_OPENAI_COMPAT_API_BASE=http://127.0.0.1:$MODEL_PORT/v1 OPENAI_COMPAT_API_KEY=x JCODE_OPENAI_COMPAT_DEFAULT_MODEL=fake-model JCODE_NO_TELEMETRY=1 JCODE_NO_AUTO_UPDATE=1"

# ---- user-level sshd acting as the cloud host -----------------------------
ssh-keygen -q -t ed25519 -N '' -f "$W/ssh/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$W/ssh/client_key"
cp "$W/ssh/client_key.pub" "$W/ssh/authorized_keys"
chmod 600 "$W/ssh/authorized_keys"
mkdir -p "$W/bin"
# The cloud "machine": own JCODE_HOME/runtime, and $REPO backed by a
# different directory (private mount namespace) so both sides really differ.
cat > "$W/cloud-shell" <<EOF
#!/usr/bin/env bash
export JCODE_HOME=$W/cloud-home JCODE_RUNTIME_DIR=$W/cloud-run JCODE_CLOUD_HOST_LABEL=cloud-tui
export PATH=$W/bin:/usr/bin
unset JCODE_SOCKET JCODE_SESSION_ID
for kv in $MODEL_ENV; do export "\$kv"; done
cmd=\${SSH_ORIGINAL_COMMAND:-bash -l}
exec unshare --user --map-root-user --mount bash -c 'mount --bind "$W/cloud-repo" "$REPO" 2>/dev/null || { mkdir -p "$REPO"; mount --bind "$W/cloud-repo" "$REPO"; }; exec bash -c "\$0"' "\$cmd"
EOF
chmod +x "$W/cloud-shell"
ln -sf "$JCODE_BIN" "$W/bin/jcode"
cat > "$W/ssh/sshd_config" <<EOF
Port $SSH_PORT
ListenAddress 127.0.0.1
HostKey $W/ssh/host_key
PidFile $W/sshd.pid
AuthorizedKeysFile $W/ssh/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
ForceCommand $W/cloud-shell
LogLevel ERROR
EOF
/usr/bin/sshd -f "$W/ssh/sshd_config" -E "$W/sshd.log"
sleep 0.5
cat > "$W/ssh/config" <<EOF
Host cloud-tui
  HostName 127.0.0.1
  Port $SSH_PORT
  User $(id -un)
  IdentityFile $W/ssh/client_key
  IdentitiesOnly yes
  StrictHostKeyChecking no
  UserKnownHostsFile $W/ssh/known_hosts
EOF
# jcode shells out to plain `ssh <host>`; point it at our config.
cat > "$W/bin-local-ssh" <<EOF
#!/usr/bin/env bash
exec /usr/bin/ssh -F "$W/ssh/config" "\$@"
EOF
mkdir -p "$W/local-bin"; mv "$W/bin-local-ssh" "$W/local-bin/ssh"; chmod +x "$W/local-bin/ssh"
if PATH="$W/local-bin:$PATH" ssh -o BatchMode=yes cloud-tui 'echo ssh-ok' > "$W/ssh-probe.txt" 2>&1 && grep -q ssh-ok "$W/ssh-probe.txt"; then pass "sshd cloud host reachable"; else cat "$W/ssh-probe.txt" "$W/sshd.log"; fail "ssh"; fi

# ---- laptop repo + TUI ------------------------------------------------------
mkdir -p "$REPO"; cd "$REPO"; git init -q -b main
git config user.name T; git config user.email t@example.invalid
echo base > file.txt; git add -A; git commit -qm init
echo "local wip" >> file.txt

LOCAL_ENV="JCODE_CLOUD_REMOTE_BINARY=$W/bin/jcode JCODE_HOME=$W/local-home JCODE_RUNTIME_DIR=$W/local-run JCODE_CLOUD_HOST=cloud-tui JCODE_CLOUD_HOST_LABEL=laptop-tui PATH=$W/local-bin:/usr/bin $MODEL_ENV"
tmux -S "$TMUX_SOCK" -f /dev/null new-session -d -s e2e -x 160 -y 45 \
  "cd $REPO && env -u JCODE_SOCKET -u JCODE_SESSION_ID $LOCAL_ENV $JCODE_BIN --no-update --no-selfdev --provider openai-compatible --model fake-model; echo EXITED; sleep 600"
wait_for() { # wait_for <regex> <seconds>
  for _ in $(seq 1 $(( $2 * 4 ))); do
    tmux -S "$TMUX_SOCK" capture-pane -p -t e2e | grep -Eq "$1" && return 0; sleep 0.25
  done; return 1
}
send() { tmux -S "$TMUX_SOCK" send-keys -t e2e -l "$1"; sleep 0.2; tmux -S "$TMUX_SOCK" send-keys -t e2e Enter; }

sleep 3
send "first message on the laptop"
wait_for '\[model\] ack' 40 && pass "local TUI turn answered" || fail "local TUI turn"

send "/cloud"
wait_for 'SSH: cloud-tui|Continuing on cloud-tui|cloud-tui' 60 && pass "TUI re-attached over SSH to cloud-tui" || fail "reattach"
wait_for 'saw_cloud_notice=True' 60 && pass "agent auto-continued on the cloud and saw the migration notice" || fail "auto-continue"
grep -q '"away_host": "cloud-tui"' "$W"/local-home/session_leases/*.json && pass "laptop lease: session away on cloud-tui" || fail "lease"
test -f "$W/cloud-repo/file.txt" && grep -q "local wip" "$W/cloud-repo/file.txt" && pass "cloud checkout has the uncommitted laptop edit" || fail "cloud repo"

# Simulate the cloud agent's work on the cloud checkout, then /local.
echo "cloud line" >> "$W/cloud-repo/file.txt"
send "/local"
wait_for 'back on this machine|Session is back' 60 || true
sleep 4
tmux -S "$TMUX_SOCK" capture-pane -p -t e2e | grep -q "SSH: cloud-tui" && fail "still attached to cloud after /local"
grep -q "cloud line" "$REPO/file.txt" && pass "cloud edit merged back into laptop checkout" || fail "merge back"
if grep -q away_host "$W"/local-home/session_leases/*.json; then fail "lease still away"; else pass "laptop lease: session home"; fi
send "and we are home"
sleep 5
python3 - "$W/model.log" <<'PY' || fail "final context"
import json, sys
last = [json.loads(l) for l in open(sys.argv[1])][-1]
flat = json.dumps(last["messages"])
for needle in ["first message on the laptop", "returned from cloud host", "and we are home"]:
    assert needle in flat, needle
print("  PASS laptop agent after /local sees the whole history incl. the return notice")
PY
echo "ALL TUI E2E CHECKS PASSED ($W)"
