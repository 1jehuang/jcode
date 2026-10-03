# Project Isolation Hardening Plan

Status: **proposed** (not started)
Last reviewed: 2026-10-02 (commit `2df1f77e9`)
Source audit: static review of the multi-project daemon (`jcode` serves sessions for
many repositories from one process). No runtime tests were run to produce this plan.

## Why this document exists

One `jcode` daemon hosts sessions for **many projects simultaneously**. "Project"
means a session working directory / repository root. Three architectural properties
make isolation fragile, and every finding below is one of them:

1. **Process-global state shared by all sessions.** A `static`, `OnceCell`, or single
   file on disk that is read by every project.
2. **No ownership check on the wire.** The protocol accepts a session id from the
   client and never compares it to the connection's own session.
3. **`working_dir: None` silently falling back to the daemon's cwd.** The daemon is
   long-lived; its cwd is whichever directory it happened to start in, which is one
   arbitrary project.

If a change touches any of these, it needs a deliberate isolation decision and a test.
See `AGENTS.md` "Project Isolation Invariants" for the rules that apply to every change.

## Status values

`pending` | `in_progress` | `blocked` | `done`

Each task lists the exact files, the acceptance criterion that proves the fix, and the
regression test that must exist before it is marked `done`.

---

## P0: Session ownership (HIGH)

These let one project's agent steer another project's session with no user action.
They are also the cheapest to fix, so do them first.

### P0.1 - `Comm*` requests trust the client-supplied `session_id`

- [ ] **Status:** pending
- **Where:** `crates/jcode-app-core/src/server/client_lifecycle.rs:2385-2436` (dispatch
  passes `req_session_id` from the wire into `handle_comm_share`),
  `crates/jcode-app-core/src/server/client_comm_context.rs:34-48` (resolves the swarm
  from that id alone).
- **Evidence:** `client_session_id` appears ~60 times in `client_lifecycle.rs` as a
  handler argument but is never compared against a request-supplied id.
- **Fix:** at dispatch, overwrite `req_session_id` with `client_session_id`, or reject
  when they differ. Apply to `CommShare`, `CommRead`, `CommMessage`, `CommList`,
  `CommListSwarms`, `CommListChannels`, `CommListModels`, and every other `Comm*`
  variant that carries a `session_id`.
- **Acceptance:** a client attached to session A that sends `CommShare` with session B's
  id gets an error (or is silently rebased to A). No state in B changes.
- **Test:** `crates/jcode-app-core/src/server/client_target_attach_tests.rs` (or a new
  `comm_ownership` module): two sessions in different temp dirs, A sends B's id,
  assert B's shared context is unchanged.

### P0.2 - `ResumeAllSessions` ignores the subscriber's working directory

- [ ] **Status:** pending
- **Where:** `crates/jcode-app-core/src/server/client_actions.rs:1086-1093` collects
  every session with a live attachment and `client_lifecycle.rs:1991-2003` dispatches
  it. The loop never consults a working directory.
- **Fix:** thread the subscriber's `working_dir` into
  `handle_resume_all_sessions` and skip sessions whose stored cwd does not match.
- **Acceptance:** resuming from project A does not deliver a continuation into any live
  session belonging to project B.
- **Test:** two live sessions in different temp dirs; resume-all from A resumes only A's.

### P0.3 - Attaching to a live session re-pins that session's working directory

- [ ] **Status:** pending
- **Where:** `client_lifecycle.rs:1925-1928` captures the resumer's cwd,
  `client_session.rs:1585` forwards it, `turn_execution.rs:1105-1106` overwrites
  `session.working_dir` and refreshes the context message.
- **Existing partial guard:** `subscribe_working_dir_replacement`
  (`client_session.rs:489-508`) rejects a report that is the home directory when the
  session already has a different cwd (issue #481). That closes the most common case
  (launching `jcode` from `$HOME`) but not the general cross-project attach.
- **Divergence note:** in the live-attach path, project-local MCP is resolved from the
  *target* session's cwd (`client_session.rs:1422-1432`) while the session's own cwd is
  overwritten from the *subscriber*. The two can disagree.
- **Fix:** when the subscriber's cwd and the target session's cwd differ, do not overwrite
  the target. Either reject the attach with a clear error or preserve the target's cwd and
  use it for MCP resolution.
- **Acceptance:** attaching from project A to a session stored under project B leaves B's
  `working_dir`, tools, MCP config, memory scope, and swarm grouping unchanged.
- **Test:** resume a session created in dir B from a client subscribed to dir A; assert
  the restored session still reports dir B.

### P0.4 - Session attach has no identity check at all

- [ ] **Status:** pending (this one may become a design decision rather than a code change)
- **Where:** `required_subscribe_working_dir` (`client_lifecycle.rs:81-89`) validates only
  that the path is absolute and non-empty; `resolve_target_subscribe_working_dir`
  (`:109-153`) returns `Ok(())` immediately at `:122-124` when the client supplies its own
  cwd, so the target session's cwd is never compared. Transport: no auth token anywhere;
  on Windows the named pipe is created without `SecurityAttributes`
  (`crates/jcode-transport/src/windows.rs:40-60`).
- **Context:** this is partly by design. jcode is a single-user local daemon and the
  session picker intentionally lists sessions across projects
  (`crates/jcode-tui/src/tui/session_picker.rs:279-291`, default filter `All`).
  Cross-project attach is therefore *allowed*; what is missing is that it is allowed
  **implicitly and without trace**.
- **Decision needed:** choose one of
  - (a) keep it permissive and make the risk explicit: log cross-project attach at warn
    level and show the other project's path in the attach confirmation; or
  - (b) require the target session's cwd to match unless the client passes an explicit
    `allow_cross_project_attach` flag; or
  - (c) issue a per-session capability token at create time and require it on attach.
- **Acceptance:** whichever option is chosen is documented, enforced, and covered by a test.
  Silent permissive attach is not an acceptable outcome.

---

## P1: Global state that crosses projects (HIGH)

### P1.1 - MCP shared pool keys entries by server name only

- [ ] **Status:** pending
- **Where:**
  - `crates/jcode-base/src/mcp/pool.rs:69-73` resolves the pool's config **once**, from
    the daemon's cwd, via `from_default_config`; the daemon initializes it at
    `crates/jcode-app-core/src/server/util.rs:62-68`.
  - `crates/jcode-base/src/mcp/pool.rs:289-291` (`begin_connect`) returns `Connected`
    when a handle with the same **name** already exists, without ever comparing configs.
  - `crates/jcode-base/src/mcp/manager.rs:204-213` hands handles to sessions filtered by
    name only.
  - `crates/jcode-base/src/mcp/pool.rs:77-98` (`connect_all`) only pools servers marked
    `shared`; `shared` defaults to true (`crates/jcode-base/src/mcp/protocol.rs:258-260`).
  - The pooled subprocess inherits the daemon's cwd (`crates/jcode-base/src/mcp/client.rs:152`),
    not the session's.
- **Impact:** project A defines a `shared` server `db` with A's env/paths/secrets; project B
  defines `db` differently. B's sessions get A's already-connected process and never start
  their own. B's agent silently talks to A's backend.
- **Fix:** key pool entries by `(config_dir, server_name)`, or store a hash of the resolved
  `McpServerConfig` and treat a hash mismatch as a different entry. Never pool a
  project-local server definition; spawn it per session with the session cwd.
- **Acceptance:** two sessions in different project dirs defining same-named `shared`
  servers with different commands each get their own process; a session whose config
  matches an existing pool entry may still reuse it.
- **Test:** in `crates/jcode-base/src/mcp/pool.rs` tests, build two pools-worth of config
  for different dirs, connect both, assert two clients and distinct commands.

### P1.2 - `session_search` has no default project scope

- [ ] **Status:** pending
- **Where:** `crates/jcode-app-core/src/tool/session_search.rs:493`
  (`working_dir_filter: params.working_dir.clone()`), schema at `:309-312`; registered as
  a base tool at `crates/jcode-app-core/src/tool/mod.rs:447-448`.
- **Impact:** an agent in project A can read transcripts (user messages, assistant text, tool
  results) from every project, unless it happens to self-restrict. `working_dir` is optional
  in the schema and nothing derives it from `ctx.working_dir`.
- **Fix:** default `working_dir_filter` to `ctx.working_dir` when the agent omits it.
  Keep an explicit opt-out for genuinely global recall (for example
  `working_dir: "*"` or `include_all_projects: true`) and say so in the schema description.
- **Also tighten:** `session_search_working_dir_matches`
  (`crates/jcode-session-types/src/lib.rs:596-616`) falls back to substring matching when
  the filter has no `/` (`:615`). A bare name like `jcode` matches every path containing
  it. Restrict the fallback to a warning or require an absolute path.
- **Acceptance:** with no `working_dir` argument, results contain only sessions whose cwd
  matches the calling session's cwd (prefix semantics, case-insensitive).
- **Test:** write sessions under two temp dirs, call the tool with only a query, assert
  only the current project's sessions come back.

### P1.3 - `bg` background tasks have no ownership enforcement

- [ ] **Status:** pending
- **Where:** `crates/jcode-app-core/src/tool/bg.rs`
  - `resolve_task_ids` (`:360-407`) returns explicit `task_id`/`task_ids` verbatim at
    `:376` and `:379`, skipping the session filter entirely.
  - `cancel` (`:628-641`) cancels without comparing `task.session_id`.
  - `list` defaults to `session_only=false` (`:518`, `filtered_tasks` at `:345-358`).
  - `cleanup` (`:644-648`) is global, sweeping every session's task files.
- **Fix:** enforce `task.session_id == ctx.session_id` unless the caller explicitly opts
  out; make `list` default to session-scoped; scope `cleanup` to the session unless
  `all_sessions=true` is passed.
- **Acceptance:** a task id belonging to another session is rejected with an explicit
  error naming the ownership mismatch.
- **Test:** create a task in session A, call `bg cancel` from session B, assert the task
  is still running.

### P1.4 - Schedules are a single global queue with no ownership check

- [ ] **Status:** pending
- **Where:** `crates/jcode-app-core/src/ambient/paths.rs:20-22` (one
  `~/.jcode/ambient/queue.json` for the whole daemon);
  `crates/jcode-app-core/src/tool/ambient.rs:925-941` (`execute_list` returns every item,
  formatted by `format_scheduled_item` at `:1003-1012`, which includes the task
  description and target session id); `execute_cancel` (`:943-962`) cancels any id without
  an ownership check.
- **Note:** execution scoping was re-verified and is **correct**. `resume_dead_session_with_reminder`
  (`ambient/runner.rs:393-402`) restores the persisted session cwd; the `Spawn` path inherits
  the parent cwd (`runner.rs:445`) and applies `item.working_dir` when present (`:479-481`).
  The only `working_dir: None` is the ambient cycle's own schedule (`tool/ambient.rs:213`),
  which is global by design. Only **listing and cancelling** leak across projects.
- **Fix:** filter `execute_list` by `item.created_by_session`; verify ownership in
  `execute_cancel`; document that the ambient agent itself is deliberately global.
- **Acceptance:** `schedule list` from project A shows only A's items; cancelling a
  schedule created by B from A fails.
- **Test:** two sessions create schedules, assert isolation of list and cancel.

---

## P2: Daemon-cwd fallbacks (MEDIUM)

`working_dir: None` must never mean "the daemon's directory".

### P2.1 - Project skills overlay falls back to the daemon cwd

- [ ] **Status:** pending
- **Where:** `crates/jcode-base/src/skill.rs:325-328` (`project_local_dir`:
  `working_dir.map(|dir| dir.join(&path)).unwrap_or(path)`, where `path` is the *relative*
  `.jcode/skills`), reached from `load_project_overlay` (`:294-302`) via
  `effective_for_working_dir` (`:309-315`).
- **Callers that can pass `None`:** `crates/jcode-app-core/src/agent.rs:490-497`
  (`current_skills_snapshot` forwards `session.working_dir`),
  `crates/jcode-app-core/src/tool/skill.rs:25-31` (from `ToolContext.working_dir`,
  set at `crates/jcode-app-core/src/agent/turn_execution.rs:975`).
- **Sessions with `working_dir: None`:** headless sessions created without a
  `create_session:<path>` command (`crates/jcode-app-core/src/server/headless.rs:59-68`),
  ambient cycle agents (`crates/jcode-app-core/src/ambient/runner.rs:400`), and
  `Agent::new` (`agent.rs:510`).
- **Impact:** a session with no cwd silently loads `.jcode/skills` from whichever project
  started the daemon.
- **Fix:** treat `None` as "global skills only" in session-scoped callers. Keep the
  process-cwd fallback only in the CLI-only `load()`/`load_for_working_dir` paths, and
  rename those so the distinction is visible at the call site.
- **Acceptance:** a session with no working dir lists no project skills, even when the
  daemon was started inside a repo that has `.jcode/skills`.

### P2.2 - Swarm prompt falls back to the daemon cwd, and the production path never passes one

- [ ] **Status:** pending
- **Where:** `crates/jcode-base/src/prompt.rs:77-78` (`load_swarm_prompt`:
  `working_dir.unwrap_or(Path::new("."))`).
- **Production path:** `crates/jcode-app-core/src/tool/mod.rs:487` registers
  `communicate::CommunicateTool::new()`, which calls `new_for_working_dir(None)`
  (`crates/jcode-app-core/src/tool/communicate.rs:1827-1829`).
  `new_for_working_dir` is called only from tests
  (`crates/jcode-app-core/src/tool/communicate_tests.rs:1140,1142`).
- **Note:** the comment directly above the registration (`tool/mod.rs:482-486`) says
  "Construct it once per session rather than sharing the process-wide instance". The code
  does the opposite: `Registry::new`/`base_tools` take no working directory, so every
  session gets the swarm prompt resolved against the daemon's cwd. The code contradicts
  its own stated intent.
- **Impact:** the `.jcode/swarm-prompt.md` of whichever project launched the daemon is
  applied to every session's swarm tool; a project's own prompt is ignored when the daemon
  started elsewhere.
- **Fix:** thread `working_dir` into `Registry::new`/`base_tools` and construct
  `CommunicateTool::new_for_working_dir(Some(cwd))` per session. That makes the existing
  comment true. Alternatively load the prompt lazily from `ToolContext.working_dir`.
- **Acceptance:** a session in project B sees B's swarm prompt even when the daemon started
  in project A.
- **Test:** extend `communicate_tests.rs:1133-1153` to construct the tool through the real
  registry path with a working dir and assert the project prompt is used.

### P2.3 - System prompt and AGENTS.md share the same fallback

- [ ] **Status:** pending
- **Where:** `crates/jcode-base/src/prompt.rs:15-16` (`load_base_system_prompt`) and
  `:1002-1003` (`load_agents_md_files_from_dir`), both using
  `working_dir.unwrap_or(Path::new("."))`.
- **Consumers:** `crates/jcode-app-core/src/agent/prompting.rs:140-151`, `agent.rs:378`,
  `agent.rs:425`.
- **Fix:** when `working_dir` is `None`, skip project-level candidates (`AGENTS.md`,
  project `system-prompt.md`) and use global/default only.
- **Acceptance:** a session with no cwd does not receive another project's AGENTS.md.

### P2.4 - `register_mcp_tools_for_dir` ignores `working_dir` in its no-pool branch (latent)

- [ ] **Status:** pending (not an active bug; see note)
- **Where:** `crates/jcode-app-core/src/tool/mod.rs:1339-1348`. The `else` branch calls
  `McpManager::new()`, which binds to `std::env::current_dir()`
  (`crates/jcode-base/src/mcp/manager.rs:122-123`), discarding the `working_dir` argument.
- **Note:** this was re-verified and is **currently inert**. All four daemon call sites
  pass `Some(mcp_pool)`: `client_session.rs:840` (subscribe), `client_session.rs:1434`
  and `:1716` (resume), `headless.rs:82`, `server.rs:958`. The bad branch is only
  reachable by a caller without a pool, and no such caller exists in production today.
- **Fix:** honor `working_dir` in the `else` branch (there is already a
  `with_shared_pool_for_dir`; add the owned equivalent taking a dir), and add a test so
  the trap cannot be walked into later.
- **Acceptance:** `register_mcp_tools_for_dir(event_tx, None, sid, Some(dir))` loads
  `.mcp.json` from `dir`.

### P2.5 - Relative paths and child shells fall back to the daemon cwd

- [ ] **Status:** pending
- **Where:** `crates/jcode-tool-core/src/lib.rs:141-149` (`resolve_path` passes relative
  paths through as-is when there is no working dir);
  `crates/jcode-app-core/src/tool/bash.rs:1025-1027` sets the child's cwd only when
  `ctx.working_dir` is `Some` (background at `:1406`);
  `crates/jcode-app-core/src/ambient/.../restart_snapshot.rs:218-225` and
  `server/comm_session.rs:77` fall back to `std::env::current_dir()`.
- **Fix:** error with a clear message instead of silently using the process cwd, or
  require the session to have a working dir before it may use relative paths.
- **Acceptance:** a session with no cwd calling `read("notes.md")` gets an actionable
  error, not the daemon's file.

---

## P3: Correctness of project identity (MEDIUM)

### P3.1 - Project hashing is unstable and unnormalized

- [ ] **Status:** pending
- **Where:** `DefaultHasher` over a raw `PathBuf`:
  - memory: `crates/jcode-base/src/memory.rs:173-181`
  - legacy notes: `crates/jcode-base/src/memory.rs:284-290`
  - goals: `crates/jcode-base/src/goal.rs:571-576`
- **Problems:**
  1. `DefaultHasher` is explicitly not stable across Rust releases. The doc comment claims
     only that it is "keyed by the absolute path" (`memory.rs:171-172`,
     `docs/MEMORY_ARCHITECTURE.md:104`), so a toolchain bump silently orphans memories and
     goals. The only mitigation is a `.bak` copy (`memory.rs:1249-1251`), not a migration.
  2. No `canonicalize`, no case folding, no trailing-slash normalization. On Windows,
     `C:\Repo`, `c:\repo`, `C:\Repo\`, and a junction pointing at `C:\Repo` produce four
     different hashes for one project, fragmenting memory and goals.
  3. A 64-bit collision would merge two projects' memory stores. Low odds, unbounded blast
     radius.
- **Fix:** add a single shared helper, for example
  `project_key(path) -> String`, that canonicalizes (falling back to lexical cleanup when
  the path does not exist), case-folds on Windows, and hashes with a stable algorithm
  (BLAKE3). Use it for memory, notes, and goals.
- **Migration:** write a manifest at the old location recording each legacy
  `DefaultHasher` name to its absolute path; on first run after the change, re-derive keys
  and move files. Keep the old files until the migration reports success.
- **Acceptance:** the same repo reached by different case or through a junction maps to one
  memory file and one goal directory.
- **Test:** create a temp dir, hash it via two path spellings, assert identical; assert the
  migration moves a legacy file and reports it.

### P3.2 - Global config is writable by any project's agent

- [ ] **Status:** pending
- **Where:** `crates/jcode-app-core/src/tool/apply_patch.rs:105-108` opens a
  `ConfigEditWatch`; `crates/jcode-app-core/src/tool/config_edit_notice.rs:1-8` states
  explicitly that agents writing `~/.jcode/config.toml` is normal workflow, and the
  "protection" only reports which keys changed. `write.rs:131-134` behaves the same.
- **Impact:** model/tool policy, MCP definitions, and auth settings live in one global file.
  Project A's agent can change the behavior of every session in the daemon. This is a
  deliberate design choice (agents are expected to configure themselves), not an oversight,
  but it means there is no isolation boundary at all at this layer.
- **Fix:** decide explicitly. Either (a) require confirmation for writes to `Config::path()`
  outside a selfdev session, or (b) keep it allowed and make the notice loud in the TUI,
  naming the keys changed and the fact that the change is daemon-wide.
- **Acceptance:** whichever option is chosen is enforced and tested.

---

## P4: Low-severity and deliberate (document, do not "fix")

These were confirmed during the audit and are recorded so they are not rediscovered as
suspected bugs later.

- [ ] **No path confinement in file tools, by design.**
  `crates/jcode-tool-core/src/lib.rs:141-149` passes absolute paths through unchanged.
  Every file tool uses it (`read.rs:155`, `write.rs:60`, `edit.rs:206`, `ls.rs:78`,
  `replace.rs:154`, `patch.rs:75,82`, `apply_patch.rs:120,132,175,219,343`). The code says
  so at `apply_patch.rs:176-180`. The only protection is the catastrophic-tier delete deny
  (`crates/jcode-command-risk/src/paths.rs:153` via `apply_patch.rs:181-190`). This is the
  "agent has the user's permissions" model, **not** a project-isolation bug. If workspace
  confinement is ever wanted, it belongs behind an explicit per-session flag, not as a
  silent default.
- [ ] **Ambient is deliberately global.** One ambient agent, reading all projects'
  sessions and memories (`crates/jcode-app-core/src/ambient/prompt.rs:173-275`,
  `ambient/runner.rs:845`), documented in `docs/AMBIENT_MODE.md`. Fine as long as it stays
  read-mostly; P1.4 is the item to watch.
- [ ] **Global memory and goal scope are explicit opt-ins.**
  `goal.rs:526-528`, `sync_goal_memory` at `goal.rs:620-647`, default recall scope `All` at
  `crates/jcode-app-core/src/tool/memory.rs`. `MemoryScope::All` never crosses into another
  project's memory (`memory.rs:678-691`), and a project write without a working dir fails
  loudly instead of silently landing elsewhere (`memory.rs:388-392`).
- [ ] **Telemetry has no transcripts by default** (`TELEMETRY.md:3`); full transcript upload
  is a separate, versioned, opt-in stream (`TELEMETRY.md:5-9`). One global consent stream
  covers all projects. Documented.
- [ ] **Debug sessions share test memory storage.** All `IsolatedTest` sessions share
  `memory/test/test_project.json` and `test_global.json` (`memory.rs:256-262`, `348-355`),
  and `clear_test_storage` wipes the whole directory (`:239-250`). Only affects debug
  sessions; acceptable unless it ever masks a real isolation bug.

---

## P5: Regression suite

Cross-cutting tests that must exist for this class of bug. Each is small and belongs next
to the code it protects, not in a separate integration bucket.

- [ ] **Session ownership:** a client cannot act on another session's id (P0.1).
- [ ] **Cwd binding:** a session created in dir B, attached from dir A, keeps dir B (P0.3).
- [ ] **Pool isolation:** same-named shared servers with different configs do not share a
  process (P1.1).
- [ ] **Search scoping:** `session_search` with no `working_dir` returns only the current
  project (P1.2).
- [ ] **Task ownership:** `bg cancel` from another session is rejected (P1.3).
- [ ] **No-cwd sessions:** with `working_dir: None`, no project skills, no project AGENTS.md,
  no project swarm prompt, and relative paths error (P2.1, P2.2, P2.3, P2.5).
- [ ] **Project key stability:** two spellings of one path hash identically (P3.1).
- [ ] **A lint or grep check** (optional but cheap) asserting no new
  `unwrap_or(Path::new("."))` / `unwrap_or_else(|| std::env::current_dir())` appears in
  session-scoped code paths without a comment explaining why. See the invariants in
  `AGENTS.md`.

---

## Explicitly out of scope

- Path confinement for tools (P4, by design).
- Making the ambient agent project-scoped (P4, by design).
- Removing the single-user, local-socket trust model (P0.4 is a decision, not a rewrite).

## Resuming this work

Read this file top to bottom. Pick the first `Status: pending` item, set it to
`in_progress`, implement it with its test, then set it to `done`. Do not batch items:
P0.1 in particular is a precondition for trusting the P0 tests, and P1.1 and P1.2 are
independent of each other and can be done in parallel by different people.
