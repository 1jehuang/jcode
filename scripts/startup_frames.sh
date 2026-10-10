#!/usr/bin/env bash
# Capture every distinct full-screen frame of a jcode TUI startup in tmux.
# Usage: startup_frames.sh <outdir> [command]
# Set STARTUP_FRAMES_ENV="VAR=value ..." to run the client with extra env
# (for example an isolated XDG_RUNTIME_DIR pointing at a test server).
out=${1:?outdir}
bin=${2:-jcode}
mkdir -p "$out"
tmux kill-session -t ovs 2>/dev/null
tmux new-session -d -s ovs -x 140 -y 40 -c "$PWD" "env ${STARTUP_FRAMES_ENV:-} $bin"
s=$(date +%s%N)
prev=""
n=0
for i in $(seq 1 ${STARTUP_FRAMES_ITERS:-200}); do
  sleep 0.005
  cur=$(tmux capture-pane -p -t ovs)
  if [ "$cur" != "$prev" ]; then
    t=$(( ($(date +%s%N) - s) / 1000000 ))
    n=$((n + 1))
    printf '%s\n' "$cur" > "$out/$(printf %02d $n)_${t}ms"
    if [ -n "$prev" ]; then
      echo "=== frame $n at ${t}ms"
      diff <(printf '%s\n' "$prev") <(printf '%s\n' "$cur") | grep '^[<>]' | sed 's/  */ /g' | head -12
    else
      echo "=== frame 1 at ${t}ms (first)"
    fi
    prev="$cur"
  fi
done
tmux kill-session -t ovs
