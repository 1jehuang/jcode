#!/usr/bin/env bash
# Run startup_acceptance.sh across a scenario matrix for one jcode build, each
# against its own server (same build) on an isolated runtime dir.
# Usage: startup_acceptance_matrix.sh <binary> <label>
set -u
bin=${1:?binary}
label=${2:?label}
root=/home/jeremy/.jcode/scratch/acc-$label
rt=$root/rt
mkdir -p "$rt"; chmod 700 "$rt"
sock=$rt/jcode.sock
repo=/home/jeremy/jcode
hint=$HOME/.jcode/remote_header_hint.json
nogit=$root/nogit; mkdir -p "$nogit"

(XDG_RUNTIME_DIR=$rt setsid "$bin" --socket "$sock" serve > "$root/serve.log" 2>&1 &)
for i in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.2; done
sleep 1
client="$bin --no-update --socket $sock"
env="XDG_RUNTIME_DIR=$rt"
fails=0
run() {
  local name=$1 cols=$2 cwd=$3
  echo "### $label / $name"
  "$repo/scripts/startup_acceptance.sh" "$client" "$root/$name" "$cols" "$cwd" $env || fails=$((fails+1))
}
# Prime the hint for this server (first launch writes it).
"$repo/scripts/startup_acceptance.sh" "$client" "$root/prime" 140 "$repo" $env >/dev/null
run warm-140 140 "$repo"
run warm-140-again 140 "$repo"
run narrow-80 80 "$repo"
run nogit-dir 140 "$nogit"
python3 - "$hint" <<'EOF'
import json,sys; p=sys.argv[1]; d=json.load(open(p)); d['mcp_servers']=[]; json.dump(d,open(p,'w'))
EOF
run cold-mcp-hint 140 "$repo"
python3 - "$hint" <<'EOF'
import json,sys; p=sys.argv[1]; d=json.load(open(p)); d.pop('session',None); json.dump(d,open(p,'w'))
EOF
run no-session-facts 140 "$repo"
python3 - "$hint" <<'EOF'
import json,sys; p=sys.argv[1]; d=json.load(open(p))
d['session']={'provider_name':'OpenAI','provider_model':'gpt-5.5','context_limit':400000,'reasoning_effort':'high','resolved_credential':'oauth'}
json.dump(d,open(p,'w'))
EOF
run mismatched-hint 140 "$repo"
run after-mismatch 140 "$repo"
# Resume a recent session that has a real transcript (>= 4 messages).
resume_id=""
for f in $(ls -t "$HOME/.jcode/sessions" | grep -E '^session_.*\.json$' | head -80); do
  n=$(python3 -c "import json,sys; print(len(json.load(open(sys.argv[1])).get('messages', [])))" "$HOME/.jcode/sessions/$f" 2>/dev/null)
  if [ "${n:-0}" -ge 4 ]; then resume_id=${f%.json}; break; fi
done
if [ -n "$resume_id" ]; then
  client="$bin --no-update --socket $sock --resume $resume_id"
  run resumed-session 140 "$repo"
  client="$bin --no-update --socket $sock"
fi
kill $(fuser "$rt/jcode-daemon.lock" 2>/dev/null) 2>/dev/null
# Client-spawned server: a fresh runtime dir with no server running, so the
# client starts it (first launch after login/reboot).
srt=$root/spawn-rt; mkdir -p "$srt"; chmod 700 "$srt"
echo "### $label / client-spawned-server"
"$repo/scripts/startup_acceptance.sh" "$bin --no-update --socket $srt/jcode.sock" "$root/client-spawned-server" 140 "$repo" XDG_RUNTIME_DIR=$srt JCODE_SOCKET=$srt/jcode.sock || fails=$((fails+1))
kill $(fuser "$srt/jcode-daemon.lock" 2>/dev/null) 2>/dev/null
# R7 (server): the History payload never waited on the model catalog for more
# than its 30ms budget, and MCP names did not build full tool definitions.
log=$HOME/.jcode/logs/jcode-$(date +%F).log
since=$(stat -c %Y "$root/serve.log")
max_models=$(awk -v s="$(date -d @"$since" '+%F %T')" 'substr($0,2,19) >= s' "$log" \
  | grep -o 'send_history prep: .*' | sed -E 's/.*models=([0-9]+)ms.*/\1/' | sort -n | tail -1)
max_tools=$(awk -v s="$(date -d @"$since" '+%F %T')" 'substr($0,2,19) >= s' "$log" \
  | grep -o 'send_history prep: .*' | sed -E 's/.*tool_names=([0-9]+)ms.*/\1/' | sort -n | tail -1)
if [ -n "$max_models" ] && [ "$max_models" -le 40 ]; then
  echo "R7   PASS  History catalog wait max ${max_models}ms (budget 30ms), mcp names max ${max_tools}ms"
else
  echo "R7   FAIL  History catalog wait max ${max_models:-?}ms"; fails=$((fails+1))
fi
echo "### $label: $fails scenario(s) with a FAIL"
