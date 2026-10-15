#!/usr/bin/env bash
# Watch logind and report whether jcode is holding the lid-switch lock.
#
# Usage: scripts/check_lid_inhibit.sh [seconds] [interval]
#
# Run it while a jcode session is working. Every poll it asks logind whether a
# jcode `handle-lid-switch` block lock exists. Any poll without the lock while
# jcode was holding it before is reported as a GAP: closing the lid at that
# instant would have suspended the machine (and dropped Wi-Fi).
set -euo pipefail

duration="${1:-180}"
interval="${2:-0.2}"

has_lock() {
  systemd-inhibit --list --no-legend 2>/dev/null |
    awk '$1 == "jcode" && $6 ~ /handle-lid-switch/ && $NF == "block" { found = 1 } END { exit !found }'
}

echo "logind HandleLidSwitch: $(busctl get-property org.freedesktop.login1 /org/freedesktop/login1 \
  org.freedesktop.login1.Manager HandleLidSwitch 2>/dev/null || echo unknown)"
echo "watching for ${duration}s (poll ${interval}s). Keep a jcode turn running."

end=$((SECONDS + duration))
held=0 gaps=0 polls=0 held_polls=0 last=""
while ((SECONDS < end)); do
  polls=$((polls + 1))
  if has_lock; then
    held_polls=$((held_polls + 1))
    [[ "$last" != held ]] && echo "$(date +%T.%N | cut -c1-12) lock held"
    held=1 last=held
  else
    if ((held)) && [[ "$last" == held ]]; then
      gaps=$((gaps + 1))
      echo "$(date +%T.%N | cut -c1-12) GAP: lock missing (lid close now would suspend)"
    fi
    last=missing
  fi
  sleep "$interval"
done

echo "polls=$polls held=$held_polls gaps=$gaps"
if ((held_polls == 0)); then
  echo "RESULT: jcode never held the lid lock. Was a turn running?"
  exit 2
fi
if ((gaps > 0)); then
  echo "RESULT: FAIL, lock dropped $gaps time(s) while jcode was working"
  exit 1
fi
echo "RESULT: PASS, lid lock held continuously while observed"
