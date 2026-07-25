#!/usr/bin/env bash
#
# One-command scoreboard for the jcode mobile app.
#
# Prints (and optionally records) every numeric gate we hill-climb on, so any
# change is a measured delta instead of a vibe:
#
#   tests        passing `swift test` count            (must not decrease)
#   e2e          mock-gateway end-to-end pipeline      (pass/fail)
#   production   App Store readiness checks            (passed/failed counts)
#   reward       UX reward over the device x scenario matrix, worst cell
#   task time    modelled seconds for key flows        (send/session/model)
#
# Usage:
#   ./TestHarness/scoreboard.sh                      # full run, human table
#   ./TestHarness/scoreboard.sh --quick              # skip e2e + full matrix
#   ./TestHarness/scoreboard.sh --save baseline.json # record for comparison
#   ./TestHarness/scoreboard.sh --compare baseline.json
#
set -uo pipefail

cd "$(dirname "$0")/.."        # ios/
HARNESS="TestHarness"
OUT_JSON=""
COMPARE=""
QUICK=""
SCRATCH="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/jcode-ios-scoreboard"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --save) OUT_JSON="$2"; shift 2 ;;
    --compare) COMPARE="$2"; shift 2 ;;
    --quick) QUICK="1"; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$SCRATCH"
log() { printf '\033[36m[score]\033[0m %s\n' "$*" >&2; }

# --- 1. swift test -----------------------------------------------------------
log "swift test"
TEST_OUT="$SCRATCH/swift-test.log"
swift test >"$TEST_OUT" 2>&1
TEST_EXIT=$?
# `grep -c` on multiple pattern hits per line stays 1-per-line; tr -d strips any
# stray whitespace so the counts stay safe to interpolate into python below.
TESTS_PASSED="$(grep -c '^✔ Test ' "$TEST_OUT" 2>/dev/null | tr -d '[:space:]')"
TESTS_FAILED="$(grep -c '^✘ Test ' "$TEST_OUT" 2>/dev/null | tr -d '[:space:]')"
TESTS_PASSED="${TESTS_PASSED:-0}"; TESTS_FAILED="${TESTS_FAILED:-0}"

# --- 2. production checks ----------------------------------------------------
log "production checks"
PROD_OUT="$SCRATCH/production.log"
./"$HARNESS"/check_production.sh >"$PROD_OUT" 2>&1
PROD_EXIT=$?
PROD_PASS="$(sed -n 's/^passed: \([0-9]*\).*/\1/p' "$PROD_OUT" | tail -1 | tr -d '[:space:]')"
PROD_FAIL="$(sed -n 's/.*failed: \([0-9]*\).*/\1/p' "$PROD_OUT" | tail -1 | tr -d '[:space:]')"
PROD_PASS="${PROD_PASS:-0}"; PROD_FAIL="${PROD_FAIL:-0}"

# --- 3. end-to-end pipeline --------------------------------------------------
E2E="skipped"
if [[ -z "$QUICK" ]]; then
  log "e2e (mock gateway + simulator)"
  if ./"$HARNESS"/run_e2e.sh >"$SCRATCH/e2e.log" 2>&1; then E2E="pass"; else E2E="FAIL"; fi
fi

# --- 4. UX reward matrix -----------------------------------------------------
log "ux reward matrix"
MATRIX_JSON="$SCRATCH/matrix.json"
REWARD_JSON="$SCRATCH/reward.json"
if [[ -n "$QUICK" ]]; then
  MATRIX_ARGS=(--devices "iPhone 17" --scenarios "empty,tool" --a11y-size "" --no-perf)
else
  MATRIX_ARGS=()
fi
python3 "$HARNESS/ui_matrix.py" "${MATRIX_ARGS[@]}" --out "$SCRATCH/shots" --json \
  >"$MATRIX_JSON" 2>"$SCRATCH/matrix.log"
( cd "$HARNESS" && python3 -m reward.aggregate --matrix-json "$MATRIX_JSON" \
    --out-json "$REWARD_JSON" >"$SCRATCH/reward.log" 2>&1 )
REWARD="$(python3 -c "
import json,sys
try:
    r=json.load(open('$REWARD_JSON'))
    print(round(r.get('reward', r.get('overall', 0)), 1))
except Exception: print(0)
")"
WORST_CELL="$(python3 -c "
import json
try:
    r=json.load(open('$REWARD_JSON'))
    cells=r.get('cells') or []
    print(round(min((c.get('reward',0) for c in cells), default=0),1))
except Exception: print(0)
")"

# --- 5. interaction cost for the flows the user cares about -----------------
log "interaction cost"
read -r COST_ACTION T_SEND T_SESSION T_MODEL <<<"$( cd "$HARNESS" && python3 -c "
from reward.interaction.engine import run_engine
try:
    r = run_engine()
    t = r.task_times_s
    print(r.expected_action_cost_s, t.get('t_send', -1), t.get('t_switch', -1), t.get('t_model', -1))
except Exception:
    print(-1, -1, -1, -1)
" 2>"$SCRATCH/taps.log" )"

# --- report ------------------------------------------------------------------
STAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
GIT_REF="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"

REPORT="$(python3 -c "
import json
print(json.dumps({
  'stamp': '$STAMP', 'git': '$GIT_REF',
  'tests_passed': int('$TESTS_PASSED' or 0),
  'tests_failed': int('$TESTS_FAILED' or 0),
  'tests_exit': int('$TEST_EXIT'),
  'e2e': '$E2E',
  'production_passed': int('$PROD_PASS'), 'production_failed': int('$PROD_FAIL'),
  'reward': float('$REWARD'), 'worst_cell': float('$WORST_CELL'),
  'cost_per_action_s': float('$COST_ACTION'),
  'secs_send': float('$T_SEND'), 'secs_switch_session': float('$T_SESSION'),
  'secs_change_model': float('$T_MODEL'),
}, indent=2))
")"

printf '\n== jcode mobile scoreboard (%s) ==\n' "$GIT_REF"
printf '  %-22s %s\n' "swift test" "$TESTS_PASSED passed, $TESTS_FAILED failed (exit $TEST_EXIT)"
printf '  %-22s %s\n' "e2e pipeline" "$E2E"
printf '  %-22s %s\n' "production checks" "$PROD_PASS passed, $PROD_FAIL failed"
printf '  %-22s %s (worst cell %s)\n' "ux reward" "$REWARD" "$WORST_CELL"
printf '  %-22s %ss/action\n' "interaction cost" "$COST_ACTION"
printf '  %-22s send=%ss switch_session=%ss change_model=%ss\n' \
  "  task times" "$T_SEND" "$T_SESSION" "$T_MODEL"
printf '\n  artifacts: %s\n' "$SCRATCH"

if [[ -n "$OUT_JSON" ]]; then
  printf '%s\n' "$REPORT" > "$OUT_JSON"
  log "saved $OUT_JSON"
fi

if [[ -n "$COMPARE" && -f "$COMPARE" ]]; then
  printf '\n== delta vs %s ==\n' "$COMPARE"
  python3 - "$COMPARE" <<PY
import json, sys
base = json.load(open(sys.argv[1]))
cur = json.loads('''$REPORT''')
# higher_is_better: True = up is good, False = down is good
keys = [
    ("tests_passed", True), ("tests_failed", False),
    ("production_passed", True), ("production_failed", False),
    ("reward", True), ("worst_cell", True),
    ("cost_per_action_s", False),
    ("secs_send", False), ("secs_switch_session", False), ("secs_change_model", False),
]
regressions = []
for k, up_good in keys:
    b, c = base.get(k), cur.get(k)
    if b is None or c is None:
        continue
    d = round(c - b, 2)
    mark = "  "
    if d != 0:
        good = (d > 0) if up_good else (d < 0)
        mark = "OK" if good else "!!"
        if not good:
            regressions.append(f"{k} {b} -> {c}")
    print(f"  {mark} {k:22} {b} -> {c}  ({d:+})")
if base.get("e2e") != cur.get("e2e"):
    print(f"     e2e                    {base.get('e2e')} -> {cur.get('e2e')}")
print()
if regressions:
    print("REGRESSIONS: " + "; ".join(regressions))
    sys.exit(1)
print("no regressions")
PY
  exit $?
fi

# Fail the run if the hard gates are broken.
[[ "$TEST_EXIT" -eq 0 ]] || exit 1
[[ "$PROD_FAIL" -eq 0 ]] || exit 1
[[ "$E2E" == "FAIL" ]] && exit 1
exit 0
