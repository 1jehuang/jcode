# Overnight Autonomous Runbook

This runbook defines the durable start, monitor, stop, and resume workflow for long autonomous sessions that coordinate background tasks, PR watches, validation, and handoff artifacts.

## Safety contract

An overnight run stops only when either:

1. every recorded success criterion is complete, or
2. the time budget has elapsed.

Do not stop early for partial success. Partial success becomes a handoff.

Never deploy, force-push, delete branches/data, expose secrets, or perform destructive cleanup without explicit approval. PR merges must follow the configured quiet-cycle protocol.

## Durable run record

Each run stores inspectable state under:

```text
.jcode/overnight-runs/<run-id>/status.md
.jcode/overnight-runs/current -> <run-id>
```

The status file should include:

- start time and run id
- stop condition
- safety constraints
- git snapshots for active repos
- active PR/watch/background task identifiers
- 30-minute watchdog checkpoints
- blockers and next safe action
- validation commands and results
- final handoff or completion report

## Start command

From the Jcode repo:

```bash
RUN_ID="overnight-$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR=".jcode/overnight-runs/$RUN_ID"
mkdir -p "$RUN_DIR"
ln -sfn "$RUN_ID" .jcode/overnight-runs/current
cat > "$RUN_DIR/status.md" <<EOF
# 12h Overnight Run

- Run id: $RUN_ID
- Start UTC: $(date -u +%Y-%m-%dT%H:%M:%SZ)
- Stop condition: all goals complete OR time budget elapsed.
- Safety: no deploy, force-push, destructive cleanup, secret exposure, or PR merge outside quiet-cycle protocol.

## Checkpoints
- $(date -u +%Y-%m-%dT%H:%M:%SZ) initialized run record.
EOF
```

Then create or update a `goal` record with the milestones and success criteria for the run.

## Monitor command

Use the helper script for a read-only checkpoint summary:

```bash
python3 scripts/overnight_status.py
```

The monitor should inspect:

- `.jcode/overnight-runs/current/status.md`
- current goal progress
- active background tasks
- PR watch state files under `.jcode/pr-feedback-watch/`
- git status for the Jcode repo and target repos
- recent validation output and blockers

Every 30 minutes, append a checkpoint like:

```text
- 2026-05-16T01:30:00Z watchdog: on_track. Active PR #30 waiting quiet cycle 1/3. Next action: poll at 01:34Z.
```

If stalled but safe to nudge, resume the bounded action or schedule it. If blocked, record the exact blocker and next required action.

## Stop command

Stopping is only allowed when all goals are complete or the time budget elapsed. Append a final section:

```text
## Final report

- End UTC: ...
- Stop reason: all_goals_complete | time_budget_elapsed
- Completed milestones: ...
- Open blockers: ...
- PRs: ...
- Validation: ...
- Next safe action: ...
```

Do not delete run artifacts.

## Resume command

Resume from the current symlink:

```bash
python3 scripts/overnight_status.py
sed -n '1,220p' .jcode/overnight-runs/current/status.md
```

Then continue the next safe action from the latest checkpoint. Before mutating code, re-check git status and active PR watch state.

## PR quiet-cycle protocol

For each bounded PR:

1. Implement a bounded change.
2. Run relevant validation.
3. Commit and open/update a PR.
4. Start a local PR watch with 3 quiet cycles and 300 second poll interval.
5. Monitor inline review comments and top-level PR comments.
6. Resolve actionable comments with commits where needed.
7. Require 3 full quiet cycles of 5 minutes each.
8. After cycle 3, wait one final 10-minute quiet period.
9. If still quiet and checks are passing or clearly non-blocking per repo policy, merge.
10. Sync local state and continue the next bounded slice.

## Bounded slice guidance

Prefer slices that are easy to validate and review independently:

- one reliability/doc/script improvement
- one UI primitive extraction
- one Storybook coverage expansion
- one visual snapshot harness
- one accessibility gate improvement

Keep generated artifacts out of commits unless explicitly intended.
