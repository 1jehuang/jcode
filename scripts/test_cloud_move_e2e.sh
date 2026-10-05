#!/usr/bin/env bash
# End-to-end test for `jcode cloud move` / `jcode cloud return`.
#
# Two fully isolated "machines" on one box, neither touching the user's
# ~/.jcode or running sessions:
#   local: JCODE_HOME=$W/local-home, repo at $REPO
#   cloud: a bubblewrap sandbox with its own HOME/JCODE_HOME and its own
#          private $REPO path (tmpfs overlay), reached through a transport
#          script standing in for `ssh <host>`.
set -euo pipefail

JCODE_BIN=${JCODE_BIN:?set JCODE_BIN to the jcode binary under test}
W=${W:-$(mktemp -d "${JCODE_SCRATCH_DIR:-/tmp}/cloud-e2e.XXXXXX")}
REPO=/tmp/jcode-cloud-e2e-repo          # identical absolute path on both machines
CLOUD_ROOT=$W/cloud-fs                   # cloud machine's private disk
mkdir -p "$W/local-home" "$CLOUD_ROOT/home" "$CLOUD_ROOT/repo"
rm -rf "$REPO"

pass() { printf '  \033[32mPASS\033[0m %s\n' "$*"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; exit 1; }
check() { local what=$1; shift; if "$@"; then pass "$what"; else fail "$what"; fi; }

# ---- fake cloud transport ------------------------------------------------
cat > "$W/cloud-transport" <<EOF
#!/usr/bin/env bash
# Usage: cloud-transport <remote shell command>. Mimics \`ssh host cmd\`.
exec bwrap --die-with-parent \\
  --ro-bind /usr /usr --ro-bind /etc /etc --symlink usr/lib /lib --symlink usr/lib64 /lib64 \\
  --symlink usr/bin /bin --symlink usr/bin /sbin \\
  --proc /proc --dev /dev --tmpfs /tmp \\
  --bind "$CLOUD_ROOT/home" /home/cloud \\
  --bind "$CLOUD_ROOT/repo" $REPO \\
  --ro-bind "$JCODE_BIN" /opt/jcode/jcode-under-test \\
  --clearenv \\
  --setenv HOME /home/cloud --setenv USER cloud --setenv PATH /usr/local/bin:/usr/bin \\
  --setenv JCODE_HOME /home/cloud/.jcode --setenv JCODE_CLOUD_HOST_LABEL cloud-e2e \\
  --setenv JCODE_NO_AUTO_UPDATE 1 --setenv JCODE_NON_INTERACTIVE 1 --setenv JCODE_NO_TELEMETRY 1 \\
  --unshare-pid --unshare-ipc --unshare-uts --hostname cloud-e2e \\
  bash -c "\$1"
EOF
chmod +x "$W/cloud-transport"

export JCODE_HOME=$W/local-home
export JCODE_CLOUD_TRANSPORT="$W/cloud-transport"
export JCODE_CLOUD_HOST_LABEL=laptop-e2e
export JCODE_NO_AUTO_UPDATE=1
export JCODE_NO_TELEMETRY=1
unset JCODE_SESSION_ID JCODE_SOCKET
J() { "$JCODE_BIN" --no-update --no-selfdev "$@"; }
CLOUD() { "$W/cloud-transport" "JCODE_HOME=/home/cloud/.jcode $*"; }
RB=(--host cloud-e2e --remote-binary /opt/jcode/jcode-under-test)

echo "== workspace: $W"

# ---- test repo with committed, staged, unstaged, untracked and ignored state
mkdir -p "$REPO"; cd "$REPO"
git init -q -b main
git config user.name "E2E Tester"; git config user.email e2e@example.invalid
printf 'fn main() {\n    println!("v1");\n}\n' > main.rs
printf 'shared line 1\nshared line 2\nshared line 3\n' > notes.txt
printf 'target/\n.env\n' > .gitignore
git add -A; git commit -qm "initial"
git remote add origin https://example.invalid/test.git
printf 'fn main() {\n    println!("v2 local wip");\n}\n' > main.rs   # unstaged
echo "staged" > staged.txt; git add staged.txt                       # staged
echo "brand new" > untracked.txt                                     # untracked
echo "SECRET_TOKEN=abc" > .env                                       # ignored but allowlisted
mkdir -p target; echo junk > target/big.bin                          # ignored build output
LOCAL_HEAD=$(git rev-parse HEAD)

# ---- a real session JSON in the isolated local home ---------------------
SID=session_e2e_$(date +%s)
mkdir -p "$JCODE_HOME/sessions" "$JCODE_HOME/todos"
python3 - "$JCODE_HOME/sessions/$SID.json" "$SID" "$REPO" <<'PY'
import json, sys, datetime
path, sid, repo = sys.argv[1:]
now = datetime.datetime.now(datetime.timezone.utc).isoformat()
def msg(i, role, blocks): return {"id": f"message_{i}", "role": role, "content": blocks, "timestamp": now}
json.dump({
  "id": sid, "title": "cloud e2e", "created_at": now, "updated_at": now,
  "working_dir": repo, "status": "Active",
  "messages": [
    msg(1, "user", [{"type": "text", "text": "Please change main.rs to print v2"}]),
    msg(2, "assistant", [{"type": "text", "text": "Editing main.rs now."},
                          {"type": "tool_use", "id": "toolu_1", "name": "bash", "input": {"command": "jcode cloud move"}}]),
  ],
}, open(path, "w"))
PY
echo '[{"id":"1","content":"finish v2","status":"in_progress","priority":"high"}]' > "$JCODE_HOME/todos/$SID.json"

echo "== 1. dry run changes nothing"
J cloud move --session "$SID" "${RB[@]}" --dry-run --allow-active --json > "$W/dry.json"
check "dry run reports dry_run"  grep -q '"dry_run": true' "$W/dry.json"
check "no local lease after dry run" test ! -e "$JCODE_HOME/session_leases/$SID.json"
check "local worktree untouched" grep -q "v2 local wip" "$REPO/main.rs"
check "local index untouched (staged.txt still staged)" bash -c "cd $REPO && git diff --cached --name-only | grep -qx staged.txt"

echo "== 2. move to cloud"
J cloud move --session "$SID" "${RB[@]}" --allow-active --json > "$W/move.json"
cat "$W/move.json"
check "local lease says away on cloud-e2e" grep -q '"away_host": "cloud-e2e"' "$JCODE_HOME/session_leases/$SID.json"
check "cloud has repo at same path" CLOUD "test -f $REPO/main.rs"
check "cloud keeps uncommitted edit" CLOUD "grep -q 'v2 local wip' $REPO/main.rs"
check "cloud keeps untracked file" CLOUD "test -f $REPO/untracked.txt"
check "cloud keeps staged file"  CLOUD "test -f $REPO/staged.txt"
check "cloud got allowlisted .env" CLOUD "grep -q SECRET_TOKEN $REPO/.env"
check "cloud did NOT get target/" CLOUD "test ! -e $REPO/target"
check "cloud HEAD == local HEAD" CLOUD "cd $REPO && test \$(git rev-parse HEAD) = $LOCAL_HEAD"
check "cloud on branch main" CLOUD "cd $REPO && test \$(git symbolic-ref --short HEAD) = main"
check "cloud keeps git identity" CLOUD "cd $REPO && test \"\$(git config user.email)\" = e2e@example.invalid"
check "cloud has transcript" CLOUD "test -f /home/cloud/.jcode/sessions/$SID.json"
check "cloud has todos" CLOUD "grep -q 'finish v2' /home/cloud/.jcode/todos/$SID.json"
check "cloud lease says here" bash -c "! grep -q away_host '$CLOUD_ROOT/home/.jcode/session_leases/$SID.json'"
CLOUD "cat /home/cloud/.jcode/sessions/$SID.json" > "$W/cloud-session.json"
python3 - "$W/cloud-session.json" <<'PY' || fail "agent-facing migration notice"
import json, sys
s = json.load(open(sys.argv[1]))
last = s["messages"][-1]
texts = [b.get("text", "") for b in last["content"] if b.get("type") == "text"]
results = [b for b in last["content"] if b.get("type") == "tool_result"]
notice = "\n".join(texts)
assert "moved from `laptop-e2e` to cloud host `cloud-e2e`" in notice, notice
assert "same path" in notice and "uncommitted" in notice, notice
assert "Build caches" in notice, notice
assert results and results[0]["tool_use_id"] == "toolu_1", "dangling tool call not closed"
assert s["migration_epoch"] == 1
print("  PASS agent-facing migration notice + dangling tool call closed")
PY

echo "== 3. local copy can no longer run or overwrite"
check "second move is refused" bash -c "! $JCODE_BIN --no-update --no-selfdev cloud move --session $SID ${RB[*]} --allow-active 2>/dev/null"
J cloud where --session "$SID" | tee "$W/where.txt"
check "where shows cloud-e2e" grep -q cloud-e2e "$W/where.txt"

echo "== 4. work happens on both sides"
# cloud agent: commits the wip + adds a new file, leaves one uncommitted edit
CLOUD "cd $REPO && git add -A && git commit -qm 'cloud: finish v2' && echo 'cloud feature' > cloud.txt && git add cloud.txt && git commit -qm 'cloud: add cloud.txt' && printf 'shared line 1\nshared line 2\nshared line 3 edited in cloud\n' > notes.txt"
# append a cloud-side turn to the transcript (what the cloud agent would do)
CLOUD "python3 - <<'PY'
import json
p='/home/cloud/.jcode/sessions/$SID.json'
s=json.load(open(p))
s['messages'].append({'id':'message_cloud','role':'assistant','content':[{'type':'text','text':'Done on the cloud: committed v2 and added cloud.txt.'}]})
json.dump(s,open(p,'w'))
PY"
# meanwhile the user keeps editing locally in a non-conflicting spot
cd "$REPO"
printf 'shared line 1 edited locally\nshared line 2\nshared line 3\n' > notes.txt
echo "local only" > local.txt

echo "== 5. return merges both sides"
J cloud return --session "$SID" "${RB[@]}" --json > "$W/return.json"
cat "$W/return.json"
check "merge applied" grep -q '"result": "applied"' "$W/return.json"
check "local branch fast-forwarded to cloud commits" bash -c "cd $REPO && git log --format=%s | grep -qx 'cloud: add cloud.txt'"
check "cloud committed file present" test -f "$REPO/cloud.txt"
check "cloud uncommitted edit merged" grep -q "edited in cloud" "$REPO/notes.txt"
check "local edit kept (same file, other hunk)" grep -q "edited locally" "$REPO/notes.txt"
check "local-only file kept" test -f "$REPO/local.txt"
check "ignored build output untouched" test -f "$REPO/target/big.bin"
check "local lease is home again" bash -c "! grep -q away_host $JCODE_HOME/session_leases/$SID.json"
check "cloud lease no longer owns" grep -q '"away_host": "returned"' "$CLOUD_ROOT/home/.jcode/session_leases/$SID.json"
python3 - "$JCODE_HOME/sessions/$SID.json" <<'PY' || fail "returned transcript"
import json, sys
s = json.load(open(sys.argv[1]))
texts = [b.get("text","") for m in s["messages"] for b in m["content"] if b.get("type")=="text"]
assert any("Done on the cloud" in t for t in texts), "cloud turn missing"
assert "returned from cloud host `cloud-e2e`" in texts[-1], texts[-1]
assert "merged into the local checkout" in texts[-1], texts[-1]
assert s["migration_epoch"] == 2
print("  PASS returned transcript has cloud turns + return notice (epoch 2)")
PY

echo "== 6. round trip again, this time with a real conflict"
J cloud move --session "$SID" "${RB[@]}" --json > /dev/null
check "second move reuses cloud checkout" CLOUD "cd $REPO && grep -q 'edited locally' notes.txt"
CLOUD "cd $REPO && printf 'CLOUD VERSION\n' > main.rs"
cd "$REPO" && printf 'LOCAL VERSION\n' > main.rs
BEFORE=$(sha256sum "$REPO/main.rs")
J cloud return --session "$SID" "${RB[@]}" --json > "$W/return2.json"
cat "$W/return2.json"
check "conflict reported" grep -q '"result": "conflicts"' "$W/return2.json"
check "conflicting file named" grep -q 'main.rs' "$W/return2.json"
check "local file untouched on conflict" test "$(sha256sum "$REPO/main.rs")" = "$BEFORE"
check "cloud work kept as ref" bash -c "cd $REPO && git cat-file -e refs/jcode-cloud/$SID/cloud:main.rs && git show refs/jcode-cloud/$SID/cloud:main.rs | grep -q 'CLOUD VERSION'"

echo
echo "ALL E2E CHECKS PASSED  (workspace $W)"
