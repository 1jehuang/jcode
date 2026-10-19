#!/usr/bin/env bash
# Agent-level e2e: a real `jcode run --resume` turn locally, `cloud move`,
# a real turn on the (sandboxed) cloud host, proof the local copy refuses to
# run, `cloud return`, and a real local turn afterward. The model is a fake
# OpenAI-compatible server that logs what the agent sent.
set -euo pipefail
JCODE_BIN=${JCODE_BIN:?}
W=$(mktemp -d "${JCODE_SCRATCH_DIR:-/tmp}/cloud-agent-e2e.XXXXXX")
REPO=/tmp/jcode-cloud-agent-e2e-repo
PORT=${PORT:-18911}
mkdir -p "$W/local-home" "$W/cloud-fs/home" "$W/cloud-fs/repo"
if [ -e "$REPO" ]; then rm -rf -- "/tmp/jcode-cloud-agent-e2e-repo"; fi

pass() { printf '  PASS %s\n' "$*"; }
fail() { printf '  FAIL %s\n' "$*"; exit 1; }

FAKE_LOG=$W/model.log FAKE_LABEL=model python3 "$(dirname "$0")/fake_openai_server.py" "$PORT" &
SRV=$!
trap 'kill $SRV 2>/dev/null || true' EXIT
sleep 1

MODEL_ENV=(JCODE_OPENAI_COMPAT_API_BASE=http://127.0.0.1:$PORT/v1 OPENAI_COMPAT_API_KEY=x JCODE_OPENAI_COMPAT_DEFAULT_MODEL=fake-model JCODE_NO_TELEMETRY=1 JCODE_NO_AUTO_UPDATE=1)

cat > "$W/cloud-transport" <<EOF
#!/usr/bin/env bash
exec bwrap --die-with-parent \\
  --ro-bind /usr /usr --ro-bind /etc /etc --symlink usr/lib /lib --symlink usr/lib64 /lib64 \\
  --symlink usr/bin /bin --symlink usr/bin /sbin --proc /proc --dev /dev --tmpfs /tmp \\
  --bind "$W/cloud-fs/home" /home/cloud --bind "$W/cloud-fs/repo" $REPO \\
  --ro-bind "$JCODE_BIN" /opt/jcode/jcode \\
  --clearenv --setenv HOME /home/cloud --setenv PATH /opt/jcode:/usr/bin \\
  --setenv JCODE_HOME /home/cloud/.jcode --setenv JCODE_CLOUD_HOST_LABEL cloud-e2e \\
  $(printf -- '--setenv %s %s ' $(printf '%s\n' "${MODEL_ENV[@]}" | tr '=' ' ')) \\
  --unshare-pid --unshare-ipc --unshare-uts --hostname cloud-e2e \\
  bash -c "\$1"
EOF
chmod +x "$W/cloud-transport"

export JCODE_HOME=$W/local-home JCODE_CLOUD_TRANSPORT=$W/cloud-transport JCODE_CLOUD_HOST_LABEL=laptop-e2e
export "${MODEL_ENV[@]}"
unset JCODE_SESSION_ID JCODE_SOCKET
RUN=("$JCODE_BIN" --no-update --no-selfdev --provider openai-compatible --model fake-model)
CLOUD() { "$W/cloud-transport" "$1"; }

mkdir -p "$REPO"; cd "$REPO"; git init -q -b main
git config user.name T; git config user.email t@example.invalid
echo hi > a.txt; git add -A; git commit -qm init

echo "== local turn"
"${RUN[@]}" run --json "turn one on the laptop" > "$W/t1.json"
SID=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["session_id"])' "$W/t1.json")
echo "   session $SID"
grep -q '\[model\] ack' "$W/t1.json" && pass "local turn produced a reply" || fail "local turn"

echo "== move"
"$JCODE_BIN" --no-update --no-selfdev cloud move --session "$SID" --host cloud-e2e --remote-binary /opt/jcode/jcode --json > "$W/move.json"
pass "moved"

echo "== local copy refuses to run"
if "${RUN[@]}" --resume "$SID" run --json "should be blocked" > "$W/blocked.json" 2> "$W/blocked.err"; then
  fail "local run was allowed after move"
fi
grep -q "moved to \`cloud-e2e\`" "$W/blocked.err" && pass "local run blocked with a clear message" || { cat "$W/blocked.err"; fail "block message"; }
REQS_BEFORE=$(wc -l < "$W/model.log")

echo "== cloud turn (continues the same conversation)"
CLOUD "cd $REPO && jcode --no-update --no-selfdev --provider openai-compatible --model fake-model --resume $SID run --json 'continue the task'" > "$W/t2.json"
grep -q 'saw_cloud_notice=True' "$W/t2.json" && pass "cloud model saw the migration notice" || { cat "$W/t2.json"; fail "cloud notice"; }
python3 - "$W/model.log" <<'PY' || fail "cloud context"
import json, sys
last = [json.loads(l) for l in open(sys.argv[1])][-1]
flat = json.dumps(last["messages"])
assert "turn one on the laptop" in flat, "earlier turn missing from cloud context"
assert "continue the task" in flat
print("  PASS cloud turn saw the full earlier conversation")
PY
CLOUD "cd $REPO && echo 'cloud edit' > b.txt"

echo "== return"
"$JCODE_BIN" --no-update --no-selfdev cloud return --session "$SID" --json > "$W/ret.json"
grep -q '"result": "applied"' "$W/ret.json" && pass "cloud edit merged back" || { cat "$W/ret.json"; fail "merge"; }
test -f "$REPO/b.txt" && pass "b.txt present locally" || fail "b.txt"

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
