#!/usr/bin/env bash
# Startup-frame acceptance checks for the jcode TUI.
#
# Launches the real client in tmux against a real server and asserts, for
# every distinct frame captured during startup:
#   R1  overscroll line is identical on the first jcode frame and the final frame
#   R2  overscroll line never shows placeholder facts (200k, raw `Oauth:`) on the
#       first frame when a matching hint exists
#   R3  the Overview widget, once shown, is still shown on the final frame
#   R4  the final frame still has the Overview after idling (+3s) and typing
#
# Usage: startup_acceptance.sh <client-cmd> <outdir> [cols] [cwd] [env...]
set -u
cmd=${1:?client command}
out=${2:?outdir}
cols=${3:-140}
cwd=${4:-$PWD}
shift 4 2>/dev/null || shift $#
extra_env="$*"
mkdir -p "$out"
sess="acc$$"
tmux kill-session -t "$sess" 2>/dev/null
tmux new-session -d -s "$sess" -x "$cols" -y 40 -c "$cwd" "env $extra_env $cmd"
prev=""
n=0
start=$(date +%s%N)
for i in $(seq 1 300); do
  sleep 0.005
  cur=$(tmux capture-pane -p -t "$sess")
  if [ "$cur" != "$prev" ]; then
    n=$((n + 1))
    t=$(( ($(date +%s%N) - start) / 1000000 ))
    printf '%s\n' "$cur" > "$out/$(printf %02d $n)_${t}ms"
    prev="$cur"
  fi
done
sleep 3
tmux capture-pane -p -t "$sess" > "$out/zz_idle"
tmux send-keys -t "$sess" h i
sleep 0.4
tmux capture-pane -p -t "$sess" > "$out/zz_typed"
tmux send-keys -t "$sess" BSpace BSpace
sleep 0.3
# R6 probe: the inline model picker (opens while typing `/model`) must list
# real models after startup. History may ship without the catalog, in which
# case GetModelCatalog has to fill it in. Do not press Enter: that would
# select the highlighted route and switch models.
tmux send-keys -t "$sess" -l "/model"
sleep 1.5
tmux capture-pane -p -t "$sess" > "$out/zz_model_picker"
tmux send-keys -t "$sess" Escape
sleep 0.2
tmux send-keys -t "$sess" C-u
sleep 0.2
tmux kill-session -t "$sess"

# Overscroll line: the last non-empty row that has the context gauge.
ovs() { grep -E '[0-9.]+[kM]/[0-9.]+[kM]' "$1" | tail -1 | sed 's/^ *//;s/ *$//'; }
frames=$(ls "$out" | grep -E '^[0-9]+_' )
first=""
for f in $frames; do
  if [ -n "$(ovs "$out/$f")" ]; then first=$f; break; fi
done
last=$(echo "$frames" | tail -1)
fail=0
report() { printf '%-4s %-5s %s\n' "$1" "$2" "$3"; [ "$2" = FAIL ] && fail=1; }

if [ -z "$first" ]; then
  report R1 FAIL "no frame with an overscroll line"
else
  a=$(ovs "$out/$first"); b=$(ovs "$out/zz_idle")
  if [ "$a" = "$b" ]; then report R1 PASS "first ($first) == settled: $a"
  else report R1 FAIL "first ($first): '$a' | settled: '$b'"; fi
  if echo "$a" | grep -qE '/200k|Oauth:'; then report R2 FAIL "placeholder on first frame: $a"
  else report R2 PASS "no placeholder on first frame"; fi
fi
seen_ov=0
for f in $frames; do grep -q 'Overview' "$out/$f" && seen_ov=1; done
if [ $seen_ov = 1 ]; then
  if grep -q Overview "$out/$last"; then report R3 PASS "Overview on final startup frame ($last)"
  else report R3 FAIL "Overview shown then gone by $last"; fi
  if grep -q Overview "$out/zz_idle" && grep -q Overview "$out/zz_typed"; then
    report R4 PASS "Overview after idle and typing"
  else report R4 FAIL "Overview missing after idle/typing"; fi
else
  report R3 N/A "no Overview content in this scenario"
  report R4 N/A ""
fi
echo "frames: $(echo $frames | wc -w)  first=$first  last=$last"
# R5: inside a git repo the first frame already carries the branch.
if git -C "$cwd" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  branch=$(git -C "$cwd" branch --show-current)
  if [ -n "$first" ] && ovs "$out/$first" | grep -q " $branch "; then
    report R5 PASS "branch '$branch' on first frame"
  else report R5 FAIL "branch '$branch' missing on first frame"; fi
else
  if [ -n "$first" ] && ! ovs "$out/$first" | grep -qE ' (master|main) '; then
    report R5 PASS "no branch outside a repo"
  else report R5 FAIL "branch shown outside a repo"; fi
fi
# R6: model picker lists models (Claude or GPT rows) and a non-trivial catalog.
picker_rows=$(grep -cE 'Opus|Sonnet|GPT|Haiku|Fable' "$out/zz_model_picker")
more=$(grep -oE '\+[0-9]+ more' "$out/zz_model_picker" | tail -1 | tr -dc 0-9)
if [ "$picker_rows" -ge 3 ] && [ "${more:-0}" -ge 10 ]; then
  report R6 PASS "model picker shows $picker_rows rows +${more} more"
else report R6 FAIL "model picker rows=$picker_rows more=${more:-0}"; fi
exit $fail
