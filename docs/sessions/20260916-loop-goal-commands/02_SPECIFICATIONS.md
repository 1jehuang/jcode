# Spec: /loop and /goal Slash Commands

## Context

jcode has rich infrastructure for scheduled tasks (`ScheduledQueue`, `ScheduleTarget`, ambient runner) and durable goals (`Goal`, `GoalStatus`, persistence) but the user-facing CLI commands are either missing or disabled:

- `/goal` → disabled stub ("disabled in this build")
- `/loop` → doesn't exist at all
- Goal subcommands (`/goal status`, `/goal complete`, etc.) → not wired

## Requirements

### /goal Command (re-enable + extend)

Re-enable the disabled `/goal` command and wire up full lifecycle management:

```
/goal <title>                  Create a new active goal (opens side panel)
/goal status                   Show all active goals with progress
/goal list [status]            List goals filtered by status
/goal update <id> <message>    Add progress update to goal
/goal complete <id>            Mark goal as completed
/goal pause <id>               Pause a goal
/goal cancel <id>              Abandon a goal
/goal resume                   Resume most recent goal in current session
/goal <id>                     Open goal detail in side panel
```

**Data model:** Already exists in `jcode-task-types` (`Goal`, `GoalStatus`, `GoalMilestone`, `GoalStep`).
**Persistence:** Already exists in `jcode-base/src/goal.rs` (JSON files in `.jcode/goals/`).
**Side panel:** Already exists in `jcode-base/src/goal.rs` (`open_goals_overview_for_session`, `open_goal_for_session`).

### /loop Command (new)

Implement recurring scheduled agent work:

```
/loop <interval> <task>        Create a recurring loop (e.g., /loop 30m check for updates)
/loop status                   Show active loops
/loop list                     List all loops with schedules
/loop cancel <id>              Cancel a loop
/loop pause <id>               Pause a loop
/loop resume <id>              Resume a paused loop
```

**Interval parsing:** Support `5m`, `30m`, `1h`, `2h`, `1d` (minutes, hours, days).
**Scheduling:** Use `ScheduledQueue` with a new `recurrence` field on `ScheduledItem`.
**Delivery:** Use `ScheduleTarget::Session` to deliver back to the originating session.
**Resume hook:** When a loop fires, inject the task as a user message into the session.

### /hooks Command (new)

Visibility into the hook system:

```
/hooks list                    Show all configured hooks and their events
/hooks status                  Show last hook execution status
/hooks test <event>            Test-fire a hook event
```

## Implementation Plan

### Phase 1: Re-enable /goal (files: `commands.rs`, `commands_dispatch.rs`)

1. Remove `handle_disabled_mission_command` stub
2. Replace with `handle_goal_command` that parses subcommands
3. Wire `/goal` into `handle_session_command` dispatch
4. Add goal creation, status, update, complete, pause, cancel handlers
5. Wire goal subcommands into the existing `crate::goal` API

### Phase 2: Implement /loop (files: new `commands_loop.rs`, `ambient.rs`)

1. Add `recurrence: Option<Recurrence>` field to `ScheduledItem`
2. Add `Recurrence` enum (`Interval(Duration)`, `Cron(String)`, `Once`)
3. After a loop item fires, re-schedule it with `scheduled_for += interval`
4. Create `commands_loop.rs` with `/loop` subcommand parsing
5. Wire into `dispatch_local_command`
6. Add loop persistence in `.jcode/loops/`

### Phase 3: Implement /hooks visibility (files: new `commands_hooks.rs`)

1. Read hook config from `config.toml` `[hooks]` section
2. Show hook status from last execution log
3. Test-fire capability for debugging

### Phase 4: Agent resume hook

1. When a loop fires, inject task as user message via `inject_message`
2. Add `loop_id` field to injected message metadata
3. Track loop execution history in `.jcode/loops/history/`

## Files to Modify

| File | Change |
|------|--------|
| `crates/jcode-tui/src/tui/app/commands.rs` | Remove disabled stub, add goal handler |
| `crates/jcode-tui/src/tui/app/commands_dispatch.rs` | Wire /goal, /loop, /hooks |
| `crates/jcode-app-core/src/ambient.rs` | Add Recurrence to ScheduledItem |
| `crates/jcode-app-core/src/ambient/persistence.rs` | Recurrence-aware scheduling |
| `crates/jcode-app-core/src/ambient/runner.rs` | Re-schedule loops after firing |
| `crates/jcode-tui/src/tui/app/commands_loop.rs` | NEW: /loop command handler |
| `crates/jcode-tui/src/tui/app/commands_hooks.rs` | NEW: /hooks command handler |

## Success Criteria

- `/goal create "Ship feature X"` creates a goal visible in side panel
- `/goal status` shows all active goals
- `/goal complete <id>` marks it completed
- `/loop 30m check for updates` creates a recurring task
- `/loop status` shows active loops with next fire time
- Loop fires inject task into session and re-schedule
- `/hooks list` shows configured hooks
- All existing tests pass
