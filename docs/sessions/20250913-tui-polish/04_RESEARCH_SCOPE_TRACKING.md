# 04 - Research: Scope/Workspace Tracking Data Sources

Data sources for the status bar: workspace name, active repos, dispatch stats.

---

## 1. Workspace / Project Name

**Struct**: `Session` in `crates/jcode-base/src/session.rs:105`

Key fields:
- `working_dir: Option<String>` -- absolute path, e.g. `/Users/k/CodeProjects/jcode`
- `short_name: Option<String>` -- memorable name like "fox"
- `id: String` -- e.g. `session_fox_1234deadbeef`

Display name (`session.rs:993`): prefers `short_name`, falls back to parsed name
from `id`, then raw `id`.

**From TUI** (`tui_state.rs:1988`): `self.session.working_dir`
**Repo name**: `Path::new(&working_dir).file_name()`

```rust
let project = self.session.working_dir
    .as_ref()
    .and_then(|d| Path::new(d).file_name())
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| "-".into());
```

---

## 2. Active Session / Repo List

### A. Workspace Map (Niri-style session grid)

**Struct**: `WorkspaceClientState` in `crates/jcode-tui/src/tui/workspace_client.rs:19`
**App field**: `workspace_client: WorkspaceClientState` (app.rs:1669)

Returns `Vec<VisibleWorkspaceRow>` where each row has:
- `workspace: i32` (vertical index)
- `sessions: Vec<WorkspaceSessionTile>` with `session_id` and `state`
  (Idle/Running/Completed/Waiting/Error/Detached)

Access (`tui_state.rs:1658`):
```rust
self.workspace_client.visible_rows(5, session_id, self.is_processing)
```

### B. Session Counts (process-level)

**Struct**: `SessionCounts` in `crates/jcode-storage/src/active_pids.rs:157`
- `total: usize` -- live sessions (PID running)
- `streaming: usize` -- actively streaming a response

Access: `session_counts()` or `user_session_counts()` (excludes swarm workers)

### C. Session Presence (per-session)

**Struct**: `SessionPresence` in `active_pids.rs:167`
- `session_id`, `pid`, `streaming`, `streaming_since`, `internal`

Access: `session_presence()` (all) / `user_session_presence()` (no internals)

---

## 3. Dispatch / Swarm Stats

### A. Swarm Members (live status)

**Struct**: `SwarmMemberStatus` in `crates/jcode-protocol/src/lib.rs:441`
**App field**: `remote_swarm_members: Vec<SwarmMemberStatus>` (app.rs:1251)

Key fields:
- `status: String` -- "ready"|"running"|"completed"|"failed"|"stopped"|"crashed"
- `task_label: Option<String>` -- stable task description
- `role: Option<String>` -- "agent" or "coordinator"
- `friendly_name: Option<String>` -- display name
- `detail: Option<String>` -- current task or error
- `todo_progress: Option<(u32, u32)>` -- (completed, total)
- `runtime: SwarmMemberRuntime` -- model, provider, effort

Updated via `ServerEvent::SwarmStatus` (`remote/server_events.rs:2153`).

### B. Plan Graph Status (DAG progress)

**Struct**: `PlanGraphStatus` in `crates/jcode-protocol/src/lib.rs:316`

Key fields:
- `item_count`, `active_ids`, `completed_ids`, `failed_ids`
- `ready_ids`, `blocked_ids`, `failed_reasons: BTreeMap<String, String>`
- `mode: String` -- "deep" or "light"

### C. Lifecycle Enum (from swarm-core)

**Enum**: `SwarmLifecycleStatus` in `crates/jcode-swarm-core/src/lib.rs:136`
Variants: Spawned, Ready, Running, RunningStale, Completed, Done, Failed,
Stopped, Crashed, Queued, Blocked, Pending, Todo

Used in `SwarmMemberRecord` (persistence); TUI compares the `String` status
from `SwarmMemberStatus` against these values.

---

## Summary

| Data | Struct | Crate/File | TUI Access |
|------|--------|------------|------------|
| Project name | `Session.working_dir` | jcode-base/session.rs:155 | `self.session.working_dir` |
| Session grid | `WorkspaceMapModel` | jcode-tui-workspace/workspace_map.rs | `self.workspace_client.visible_rows(...)` |
| Session counts | `SessionCounts` | jcode-storage/active_pids.rs:157 | `session_counts()` / `user_session_counts()` |
| Swarm members | `SwarmMemberStatus` | jcode-protocol/lib.rs:441 | `self.remote_swarm_members` |
| Plan progress | `PlanGraphStatus` | jcode-protocol/lib.rs:316 | via server events |
| Lifecycle states | `SwarmLifecycleStatus` | jcode-swarm-core/lib.rs:136 | string-compared from status field |

## Combined Snippet for Status Bar

```rust
fn render_status_bar(&self) {
    // 1. Project name
    let project = self.session.working_dir.as_ref()
        .and_then(|d| Path::new(d).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "-".into());

    // 2. Sessions in workspace
    let rows = self.workspace_map_rows();
    let session_count: usize = rows.iter().map(|r| r.sessions.len()).sum();

    // 3. Swarm stats
    let m = &self.remote_swarm_members;
    let active = m.iter().filter(|x| x.status == "running").count();
    let done   = m.iter().filter(|x| x.status == "completed").count();
    let failed = m.iter().filter(|x| x.status == "failed").count();

    // 4. Global presence
    let counts = jcode_storage::user_session_counts();

    // e.g. "jcode | 3 panes | 2 active 1 done | 4 total/1 streaming"
}
```
