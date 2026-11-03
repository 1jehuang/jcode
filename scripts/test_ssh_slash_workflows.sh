#!/usr/bin/env bash
# Real native SSH + TUI + daemon acceptance. Only model responses are synthetic.
# Never reads the operator's auth/config or mutates their daemon/workspace.
set -euo pipefail
: "${JCODE_BIN:?Set JCODE_BIN to the rebuilt binary}"
JCODE_BIN=$(realpath "$JCODE_BIN")
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
# Viewer intentionally refuses all paths beneath .jcode, including agent scratch.
# These small, short-lived fixtures therefore live in the private runtime directory.
W=$(mktemp -d "${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/ssh-slash-workflows.XXXXXX")
for prerequisite in ssh sshd ssh-keygen tmux python3; do
    command -v "$prerequisite" >/dev/null || { echo "Missing $prerequisite" >&2; exit 1; }
done
mkdir -p "$W"/{local-home,remote-home,local-run,remote-run,ssh,bin,project,local-project,frames,control}
# Avoid fixed ports, including collisions with other concurrent acceptance tests.
read -r SSH_PORT MODEL_PORT < <(python3 - <<'PY'
import socket
sockets = [socket.socket(), socket.socket()]
for s in sockets: s.bind(('127.0.0.1', 0))
print(*(s.getsockname()[1] for s in sockets))
PY
)
TMUX_SOCK=$W/tmux.sock
cleanup() {
    tmux -S "$TMUX_SOCK" kill-server 2>/dev/null || true
    [[ ! -f $W/ssh/pid ]] || kill "$(cat "$W/ssh/pid")" 2>/dev/null || true
    [[ -z ${MODEL_PID:-} ]] || kill "$MODEL_PID" 2>/dev/null || true
    env -i PATH="$PATH" HOME="$W/remote-home" JCODE_HOME="$W/remote-home/.jcode" \
        JCODE_RUNTIME_DIR="$W/remote-run" "$JCODE_BIN" --no-update server stop >/dev/null 2>&1 || true
    echo "Acceptance artifacts: $W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; cat "$W/frame.txt" >&2 2>/dev/null || true; exit 1; }
ssh-keygen -q -t ed25519 -N '' -f "$W/ssh/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$W/ssh/client_key"
cp "$W/ssh/client_key.pub" "$W/ssh/authorized_keys"
printf '[127.0.0.1]:%s %s\n' "$SSH_PORT" "$(cut -d ' ' -f 1,2 "$W/ssh/host_key.pub")" > "$W/ssh/known_hosts"
cat > "$W/ssh/config" <<EOF
Host slash-fixture
    HostName 127.0.0.1
    Port $SSH_PORT
    User $(id -un)
    IdentityFile $W/ssh/client_key
    IdentitiesOnly yes
    StrictHostKeyChecking yes
    UserKnownHostsFile $W/ssh/known_hosts
EOF
cat > "$W/bin/ssh" <<EOF
#!/bin/sh
exec /usr/bin/ssh -F '$W/ssh/config' "\$@"
EOF
cat > "$W/remote-shell" <<EOF
#!/bin/sh
exec env -i PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 HOME='$W/remote-home' \\
 JCODE_HOME='$W/remote-home/.jcode' JCODE_RUNTIME_DIR='$W/remote-run' \\
 JCODE_NO_TELEMETRY=1 JCODE_NO_AUTO_UPDATE=1 JCODE_NON_INTERACTIVE=1 \\
 JCODE_OPENAI_COMPAT_API_BASE='http://127.0.0.1:$MODEL_PORT/v1' \\
 OPENAI_COMPAT_API_KEY=fixture-only JCODE_OPENAI_COMPAT_DEFAULT_MODEL=fake-model \\
 /bin/bash -c "\${SSH_ORIGINAL_COMMAND:?}"
EOF
chmod +x "$W/bin/ssh" "$W/remote-shell"
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
FAKE_LOG="$W/model.log" FAKE_LABEL=workflow FAKE_CONTROL_DIR="$W/control" \
    python3 "$SCRIPT_DIR/fake_openai_server.py" "$MODEL_PORT" > "$W/model-output.log" 2>&1 &
MODEL_PID=$!
PATH="$W/bin:$PATH" ssh -o BatchMode=yes slash-fixture 'printf fixture-ready' > "$W/probe"
grep -Fq fixture-ready "$W/probe" || fail 'real SSH authentication failed'
printf 'host-file-sentinel\n' > "$W/project/host.txt"
printf 'client-file-sentinel\n' > "$W/local-project/host.txt"
# Different client bytes prove commands did not accidentally use laptop paths.
mkdir -p "$W/local-home/.jcode"
printf '[display]\ncentered = false\n' > "$W/local-home/.jcode/config.toml"
cp "$W/local-home/.jcode/config.toml" "$W/local-config-before"
printf -v launch '%q ' env -i "PATH=$W/bin:/usr/local/bin:/usr/bin:/bin" LANG=C.UTF-8 \
    TERM=xterm-256color "HOME=$W/local-home" "JCODE_HOME=$W/local-home/.jcode" \
    "JCODE_RUNTIME_DIR=$W/local-run" JCODE_NO_TELEMETRY=1 JCODE_NO_AUTO_UPDATE=1 \
    "$JCODE_BIN" --no-update --no-selfdev --ssh slash-fixture --ssh-binary "$JCODE_BIN" \
    --remote-working-dir "$W/project"
printf -v local_workspace '%q' "$W/local-project"
tmux -S "$TMUX_SOCK" -f /dev/null new-session -d -s workflows -x 160 -y 55 "cd $local_workspace && exec $launch"
wait_for() {
    local text=$1
    for ((i=0; i<160; i++)); do
        tmux -S "$TMUX_SOCK" capture-pane -p -t workflows > "$W/frame.txt" || fail 'client exited'
        if grep -Fq "$text" "$W/frame.txt"; then return 0; fi
        sleep 0.25
    done
    fail "missing UI text: $text"
}
command_ui() {
    tmux -S "$TMUX_SOCK" send-keys -t workflows -l "$1"
    tmux -S "$TMUX_SOCK" send-keys -t workflows Enter
    wait_for "$2"
    cp "$W/frame.txt" "$W/frames/$3.txt"
    echo "JCODE_CHECKPOINT {\"message\":\"$3 verified\"}"
}
wait_for 'api-key:openai-compatible'
command_ui '/file host.txt' 'host-file-sentinel' host-file
command_ui '/config set display.centered true' 'display.centered' host-config
command_ui '/account default-model fake-model' 'Saved default model on runtime host' account-default
command_ui '/account openai settings' 'settings on runtime host' account-settings
command_ui '/reset usage limits openai cancel' 'Reset confirmation cleared' reset-cancel
command_ui '/account openai remove fixture-review-only' 'Remove openai account fixture-review-only' removal-review
command_ui '/swarm-prompt set project isolated-prompt-marker' 'Saved host swarm prompt' host-prompt
command_ui '/fast default off' 'Host fast default: off' host-fast
command_ui '/log mark isolated-log-marker' 'Host log marker written' host-log
command_ui '/save ssh-workflow-bookmark' 'ssh-workflow-bookmark' bookmark
command_ui '/sessions saved' 'Remote sessions' sessions
PARENT=$(python3 - "$W" <<'PY'
import json, pathlib, sys
for p in (pathlib.Path(sys.argv[1])/'remote-home/.jcode/sessions').glob('*.json'):
    s=json.loads(p.read_text())
    if s.get('saved'): print(s['id']); break
else: raise AssertionError('bookmark was not persisted on the host')
PY
)
tmux -S "$TMUX_SOCK" send-keys -t workflows -l '/fork'
tmux -S "$TMUX_SOCK" send-keys -t workflows Enter
CHILD=''
for ((i=0; i<120; i++)); do
    CHILD=$(python3 - "$W" "$PARENT" <<'PY'
import json, pathlib, sys
for p in (pathlib.Path(sys.argv[1])/'remote-home/.jcode/sessions').glob('*.json'):
    s=json.loads(p.read_text())
    if s.get('parent_id') == sys.argv[2]: print(s['id']); break
PY
)
    [[ -z $CHILD ]] || break
    sleep 0.25
done
[[ -n $CHILD ]] || fail 'no persisted remote child'
# Query after the navigation response has loaded, not while /fork is in flight.
command_ui '/session' "$CHILD" fork
command_ui '/back' "$PARENT" back
command_ui '/session' "$PARENT" back-identity
command_ui '/poke on' 'Remote poke enabled' poke-on
command_ui '/poke off' 'Stopped /poke at the next turn boundary' poke-off
command_ui '/fix' 'Recovered remote runtime context' recovery
command_ui '/improve plan isolated-workflow-marker [e2e:hold:slashloop]' 'Started remote /improve planning workflow' workflow-start
# Hold a real in-flight model request. Stop must not wait for its Agent lock.
for ((i=0; i<120; i++)); do
    [[ ! -f $W/control/slashloop.held ]] || break
    sleep 0.25
done
[[ -f $W/control/slashloop.held ]] || fail 'workflow did not reach held provider request'
command_ui '/improve stop' 'Stopped /improve at the next turn boundary' workflow-stop
# Stop disarms continuation, but must not cancel the current turn.
touch "$W/control/slashloop.release"
wait_for '[workflow] ack.'
command_ui '/debug-visual on' 'Visual debug' client-presentation
command_ui '/file /proc/self/environ' 'Remote viewing does not expose device or process files' protected-file
if grep -Fq 'Ctrl+Y' "$W/frame.txt"; then fail 'command failure generated inference fallback'; fi
tmux -S "$TMUX_SOCK" send-keys -t workflows -l '/client-reload'
tmux -S "$TMUX_SOCK" send-keys -t workflows Enter
# exec keeps its PID. Its argv must change to the explicit remote --resume target.
CLIENT_PID=$(tmux -S "$TMUX_SOCK" list-panes -F '#{pane_pid}')
RELOADED=false
for ((i=0; i<120; i++)); do
    if python3 - "$CLIENT_PID" "$PARENT" <<'PY'
import pathlib, sys
args=pathlib.Path('/proc',sys.argv[1],'cmdline').read_bytes().split(b'\0')
sys.exit(0 if b'--resume' in args and sys.argv[2].encode() in args else 1)
PY
    then RELOADED=true; break; fi
    sleep 0.25
done
[[ $RELOADED == true ]] || fail 'client did not re-exec with the original remote identity'
# Wait for the new client's transport bootstrap, not the previous screen contents.
BOOTSTRAPPED=false
for ((i=0; i<120; i++)); do
    if grep -Fq 'resumed=true, request=subscribe_resume' "$W/local-home/.jcode/logs/"*.log; then
        BOOTSTRAPPED=true
        break
    fi
    sleep 0.25
done
[[ $BOOTSTRAPPED == true ]] || fail 'reloaded client did not subscribe to the remote session'
wait_for '[workflow] ack.'
command_ui '/file host.txt' 'host-file-sentinel' after-reload
command_ui '/session' "$PARENT" reload-identity
cmp "$W/local-config-before" "$W/local-home/.jcode/config.toml" || fail 'client config mutated'
python3 - "$W" <<'PY'
import json, pathlib, sys, tomllib
w = pathlib.Path(sys.argv[1])
config = tomllib.loads((w/'remote-home/.jcode/config.toml').read_text())
assert config['display']['centered'] is True, config
assert config['provider']['default_model'] == 'fake-model', config
assert config['provider']['openai_service_tier'] == 'off', config
assert (w/'project/.jcode/swarm-prompt.md').read_text() == 'isolated-prompt-marker'
loop_files = list((w/'remote-home/.jcode/remote-loops').glob('*.json'))
assert loop_files, 'workflow state was not persisted'
assert all(not json.loads(p.read_text()).get('workflow', {}).get('active', False) for p in loop_files)
assert any('isolated-log-marker' in p.read_text() for p in (w/'remote-home/.jcode/logs').glob('*.log'))
sessions = [json.loads(p.read_text()) for p in (w/'remote-home/.jcode/sessions').glob('*.json')]
assert any(s.get('parent_id') for s in sessions), 'fork did not persist a remote child'
assert (w/'model.log').exists(), 'workflow never reached the model'
rows = [json.loads(line) for line in (w/'model.log').read_text().splitlines()]
assert any('isolated-workflow-marker' in json.dumps(row) for row in rows), rows
assert sum(row['agent'] for row in rows) == 1, 'control commands accidentally started another model turn'
assert (w/'local-project/host.txt').read_text() == 'client-file-sentinel\n'
assert not list((w/'local-home/.jcode/sessions').glob('*.json')), 'remote sessions saved locally'
PY
printf 'PASS: native SSH account, host config/file, session navigation/bookmark, loop/recovery and client reload workflows\n'
