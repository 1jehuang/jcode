#!/usr/bin/env bash
# Agent-level e2e: a real `jcode run --resume` turn locally, `cloud move`,
# a real turn on the (sandboxed) cloud host, proof the local copy refuses to
# run, `cloud return`, and a real local turn afterward. The model is a fake
# OpenAI-compatible server that logs what the agent sent.
set -euo pipefail
# Re-exec with an allowlisted environment before starting any client or daemon.
# Even a caller-supplied W is only a parent: never reuse or delete an old run.
if [[ ${1:-} != --isolated ]]; then
  JCODE_BIN=$(realpath -e "${JCODE_BIN:?set JCODE_BIN to the jcode binary under test}")
  SCRIPT=$(realpath -e "$0")
  W=$(mktemp -d "${W:-${JCODE_SCRATCH_DIR:-/tmp}}/cloud-test.XXXXXX")
  mkdir -m 700 "$W/home" "$W/runtime" "$W/tmp"
  EXTRA=()
  for name in CLOUD_JCODE_BIN RETURN_JCODE_BIN CLOUD_UPGRADE_TO; do
    if [[ -n ${!name:-} ]]; then EXTRA+=("$name=$(realpath -e "${!name}")"); fi
  done
  exec env -i PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 \
    HOME="$W/home" TMPDIR="$W/tmp" XDG_RUNTIME_DIR="$W/runtime" JCODE_RUNTIME_DIR="$W/runtime" \
    JCODE_TEST_WORKSPACE="$W" W="$W" JCODE_BIN="$JCODE_BIN" \
    "${EXTRA[@]}" bash "$SCRIPT" --isolated
fi
REPO=$W/repo
printf -v REPO_Q %q "$REPO"
# Stop only processes carrying this run's unguessable workspace marker. This
# includes detached daemons and provider children, not the user's real server.
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  env -u JCODE_TEST_WORKSPACE timeout --kill-after=2s 12s python3 - "$W" "$$" <<'PY' || status=1
import os, signal, sys, time
marker = b'JCODE_TEST_WORKSPACE=' + os.fsencode(sys.argv[1])
exclude = {os.getpid(), int(sys.argv[2])}
def owned():
    found = []
    for name in os.listdir('/proc'):
        if not name.isdigit() or int(name) in exclude:
            continue
        try:
            if marker in open('/proc/' + name + '/environ', 'rb').read().split(b'\0'):
                found.append(int(name))
        except (OSError, ProcessLookupError):
            pass
    return found
def stop(sig):
    for pid in owned():
        try: os.kill(pid, sig)
        except ProcessLookupError: pass
stop(signal.SIGTERM)
deadline = time.monotonic() + 5
while owned() and time.monotonic() < deadline:
    time.sleep(.1)
stop(signal.SIGKILL)
deadline = time.monotonic() + 2
while owned() and time.monotonic() < deadline:
    time.sleep(.1)
remaining = owned()
if remaining:
    print('Cleanup failed, surviving test processes:', remaining, file=sys.stderr)
    sys.exit(1)
PY
  if (( status != 0 )); then echo "FAILED: artifacts retained at $W" >&2; fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$W/local-home" "$W/cloud-fs/home" "$W/cloud-fs/repo"

pass() { printf '  PASS %s\n' "$*"; }
fail() { printf '  FAIL %s\n' "$*"; exit 1; }

# Bind port zero inside the real fake server, then publish the selected port.
# This avoids the pick-a-free-port / close / rebind race in concurrent runs.
FAKE_LOG=$W/model.log FAKE_LABEL=model python3 - "$(dirname "$0")/fake_openai_server.py" "$W/port" > "$W/provider.log" 2>&1 <<'PY' &
import http.server, pathlib, runpy, sys
script, port_file = sys.argv[1:]
original = http.server.HTTPServer.server_activate
def activate(server):
    original(server)
    pathlib.Path(port_file).write_text(str(server.server_address[1]))
http.server.HTTPServer.server_activate = activate
sys.argv = [script, '0']
runpy.run_path(script, run_name='__main__')
PY
SRV=$!
for ((attempt=0; attempt<100; attempt++)); do
  [[ -s "$W/port" ]] && break
  kill -0 "$SRV" 2>/dev/null || { cat "$W/provider.log"; fail "provider exited during startup"; }
  sleep .1
done
[[ -s "$W/port" ]] || fail "provider startup timed out"
PORT=$(cat "$W/port")

MODEL_ENV=("JCODE_OPENAI_COMPAT_API_BASE=http://127.0.0.1:$PORT/v1" OPENAI_COMPAT_API_KEY=x JCODE_OPENAI_COMPAT_DEFAULT_MODEL=fake-model JCODE_NO_TELEMETRY=1 JCODE_NO_AUTO_UPDATE=1)

{
  printf '#!/usr/bin/env bash\n'
  printf 'CLOUD_HOME=%q\nCLOUD_REPO=%q\nREPO=%q\nBINARY=%q\nWORKSPACE=%q\n' \
    "$W/cloud-fs/home" "$W/cloud-fs/repo" "$REPO" "$JCODE_BIN" "$W"
  printf 'MODEL_PORT=%q\n' "$PORT"
  cat <<'EOF'
exec bwrap --die-with-parent \
  --ro-bind /usr /usr --ro-bind /etc /etc --symlink usr/lib /lib --symlink usr/lib64 /lib64 \
  --symlink usr/bin /bin --symlink usr/bin /sbin --proc /proc --dev /dev --tmpfs /tmp \
  --bind "$CLOUD_HOME" /home/cloud --bind "$CLOUD_REPO" "$REPO" \
  --ro-bind "$BINARY" /opt/jcode/jcode \
  --clearenv --setenv HOME /home/cloud --setenv USER cloud --setenv PATH /opt/jcode:/usr/local/bin:/usr/bin \
  --setenv JCODE_HOME /home/cloud/.jcode --setenv JCODE_CLOUD_HOST_LABEL cloud-e2e \
  --setenv JCODE_RUNTIME_DIR /home/cloud/runtime --setenv XDG_RUNTIME_DIR /home/cloud/runtime \
  --setenv JCODE_TEST_WORKSPACE "$WORKSPACE" \
  --setenv JCODE_NO_AUTO_UPDATE 1 --setenv JCODE_NON_INTERACTIVE 1 --setenv JCODE_NO_TELEMETRY 1 \
  --setenv JCODE_OPENAI_COMPAT_API_BASE "http://127.0.0.1:$MODEL_PORT/v1" \
  --setenv OPENAI_COMPAT_API_KEY x --setenv JCODE_OPENAI_COMPAT_DEFAULT_MODEL fake-model \
  --unshare-pid --unshare-ipc --unshare-uts --hostname cloud-e2e \
  bash -c "$1"
EOF
} > "$W/cloud-transport"
chmod +x "$W/cloud-transport"

export PATH="$W:$PATH"
export JCODE_HOME=$W/local-home JCODE_CLOUD_TRANSPORT=cloud-transport JCODE_CLOUD_HOST_LABEL=laptop-e2e
export "${MODEL_ENV[@]}"
unset JCODE_SESSION_ID JCODE_SOCKET
RUN=(timeout --kill-after=5s 90s "$JCODE_BIN" --no-update --no-selfdev --provider openai-compatible --model fake-model)
CLOUD() { timeout --kill-after=5s 90s "$W/cloud-transport" "$1"; }

mkdir -p "$REPO"; cd "$REPO"; git init -q -b main
git config user.name T; git config user.email t@example.invalid
echo hi > a.txt; git add -A; git commit -qm init

echo "== local turn"
"${RUN[@]}" run --json "turn one on the laptop" > "$W/t1.json"
SID=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["session_id"])' "$W/t1.json")
echo "   session $SID"
if grep -q '\[model\] ack' "$W/t1.json"; then pass "local turn produced a reply"; else fail "local turn"; fi

echo "== move"
timeout --kill-after=5s 90s "$JCODE_BIN" --no-update --no-selfdev cloud move --session "$SID" --host cloud-e2e --remote-binary /opt/jcode/jcode --json > "$W/move.json"
pass "moved"

echo "== local copy refuses to run"
REQS_BEFORE=$(wc -l < "$W/model.log")
if "${RUN[@]}" --resume "$SID" run --json "should be blocked" > "$W/blocked.json" 2> "$W/blocked.err"; then
  fail "local run was allowed after move"
fi
if grep -q "moved to \`cloud-e2e\`" "$W/blocked.err"; then
  pass "local run blocked with a clear message"
else
  cat "$W/blocked.err"; fail "block message"
fi
[[ $(wc -l < "$W/model.log") == "$REQS_BEFORE" ]] || fail "blocked run contacted provider"

echo "== cloud turn (continues the same conversation)"
CLOUD "cd $REPO_Q && jcode --no-update --no-selfdev --provider openai-compatible --model fake-model --resume $SID run --json 'continue the task'" > "$W/t2.json"
if grep -q 'saw_cloud_notice=True' "$W/t2.json"; then
  pass "cloud model saw the migration notice"
else
  cat "$W/t2.json"; fail "cloud notice"
fi
python3 - "$W/model.log" <<'PY' || fail "cloud context"
import json, sys
last = [json.loads(l) for l in open(sys.argv[1])][-1]
flat = json.dumps(last["messages"])
assert "turn one on the laptop" in flat, "earlier turn missing from cloud context"
assert "continue the task" in flat
print("  PASS cloud turn saw the full earlier conversation")
PY
CLOUD "cd $REPO_Q && echo 'cloud edit' > b.txt"

echo "== return"
timeout --kill-after=5s 90s "$JCODE_BIN" --no-update --no-selfdev cloud return --session "$SID" --json > "$W/ret.json"
if grep -q '"result": "applied"' "$W/ret.json"; then
  pass "cloud edit merged back"
else
  cat "$W/ret.json"; fail "merge"
fi
if test -f "$REPO/b.txt"; then pass "b.txt present locally"; else fail "b.txt"; fi

echo "== local turn after return"
"${RUN[@]}" --resume "$SID" run --json "back on the laptop" > "$W/t3.json"
python3 - "$W/model.log" <<'PY' || fail "local context after return"
import json, sys
last = [json.loads(l) for l in open(sys.argv[1])][-1]
flat = json.dumps(last["messages"])
for needle in ["turn one on the laptop", "continue the task", "returned from cloud host", "back on the laptop"]:
    assert needle in flat, needle
assert "should be blocked" not in flat, "blocked prompt leaked into transcript"
print("  PASS local turn sees laptop turn + cloud turn + return notice, no blocked prompt")
PY
echo "ALL AGENT E2E CHECKS PASSED ($W)"
