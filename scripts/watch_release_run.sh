#!/usr/bin/env bash
# Watch a GitHub Actions run to completion and report the outcome.
#
# Used to own a release end to end: tagging is not "released", so this polls the
# run and exits non-zero if it fails, printing the failed jobs so the cause is
# visible without opening a browser.
set -uo pipefail

RUN_ID="${1:?usage: watch_release_run.sh <run-id> [max-polls] [sleep-seconds]}"
MAX_POLLS="${2:-100}"
SLEEP_S="${3:-30}"

for i in $(seq 1 "$MAX_POLLS"); do
    line="$(gh run view "$RUN_ID" --json status,conclusion -q '"\(.status) \(.conclusion)"' 2>&1)" || line="unknown"
    printf 'JCODE_PROGRESS {"message":"run %s: %s","current":%d,"total":%d,"unit":"polls"}\n' \
        "$RUN_ID" "$line" "$i" "$MAX_POLLS"
    case "$line" in
        "completed success")
            echo "RUN SUCCESS"
            exit 0
            ;;
        "completed failure" | "completed cancelled" | "completed timed_out")
            echo "RUN FAILED: $line"
            echo "--- failed jobs ---"
            gh run view "$RUN_ID" --json jobs \
                -q '.jobs[] | select(.conclusion != "success" and .conclusion != "skipped") | "\(.conclusion)\t\(.name)"' 2>&1 | head -20
            exit 1
            ;;
    esac
    sleep "$SLEEP_S"
done

echo "RUN STILL RUNNING after $MAX_POLLS polls"
exit 2
