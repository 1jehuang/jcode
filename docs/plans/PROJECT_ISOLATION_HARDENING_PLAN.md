# Project Isolation Hardening Plan

Status: **in_progress** (P0.1-P0.4, P1.1-P1.4, P2.1 done)
Last reviewed: 2026-10-04 (P2.1 shipped)
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

### Known blocker: `cargo test -p jcode-app-core --lib` does not finish

Not an isolation bug, but it blocks whole-suite verification of every task here.

`server::comm_control` tests take the process-global env lock
(`crates/jcode-base/src/storage.rs::lock_test_env`) and hold it across an `.await`. A test
that wins the lock and then blocks on something else wedges the run: every later
lock-waiter queues behind it and the suite stops making progress with no error. Which test
is reported as hung varies between runs (`assign_next_prefers_worker_with_matching_subsystem_metadata`,
then `assign_task_allows_taking_over_stale_assignment`, then
`approve_plan_rejects_proposal_that_forms_cycle_with_existing_plan`), which is the
signature of the *waiters* being reported, not the blocker. Each passes in isolation.

Workaround used for now: verify each touched module in its own process, for example
`cargo test -p jcode-app-core --lib comm_ownership`. A real fix belongs in the test
harness, not in any isolation task.

---

## P0: Session ownership (HIGH)

These let one project's agent steer another project's session with no user action.
They are also the cheapest to fix, so do them first.

### P0.1 - `Comm*` requests trust the client-supplied `session_id`

- [x] **Status:** done
- **Where:** `crates/jcode-app-core/src/server/comm_auth.rs` (new; ownership proof for both
  transport paths), called from
  `crates/jcode-app-core/src/server/client_lifecycle.rs` (subscribed path) and
  `crates/jcode-app-core/src/server/client_lightweight_control.rs` (one-shot control path).
  `crates/jcode-app-core/src/tool/communicate/transport.rs` now attaches the capability
  the one-shot path requires.
- **Evidence:** `client_session_id` appears ~60 times in `client_lifecycle.rs` as a
  handler argument but is never compared against a request-supplied id.
- **Fix:** every `Comm*` request must prove ownership of the session it names before
  dispatch. Subscribed connections compare against the `client_session_id` the daemon
  minted for that connection. One-shot control connections (`tool/communicate.rs` opens
  one per request, before any `Subscribe`, so there is no session id to compare) carry a
  capability minted in-process from a per-daemon secret; `jcode-transport` exposes no
  portable peer-credential API on either Unix sockets or Windows named pipes, so process
  identity cannot be checked directly. The capability is verified against the id the
  request *claims*, not against a known session, which is what blocks replay of a token
  for session A against session B.
- **Also fixed:** a rejected one-shot request used to close the connection silently, which
  is indistinguishable from a daemon crash. It now emits `ServerEvent::Error` on the
  request's own id.
- **Acceptance:** a client attached to session A that sends `CommShare` with session B's
  id gets an error. No state in B changes.
- **Test:** `crates/jcode-app-core/src/server/comm_ownership_tests.rs` (4 socket-level
  tests) and `comm_auth_tests.rs` (22 tests, including one that scans
  `crates/jcode-protocol/src/wire.rs` for the `Comm*` variant list so a new variant cannot
  bypass the check).

### P0.2 - `ResumeAllSessions` ignores the subscriber's working directory

- [x] **Status:** done (commit `153ac7eb0`)
- **Where:** `crates/jcode-app-core/src/server/client_actions.rs` `handle_resume_all_sessions`
  collected every session with a live attachment and `client_lifecycle.rs:2014-2025`
  dispatched it. The loop never consulted a working directory.
- **Fix:** `handle_resume_all_sessions` takes a `caller_working_dir: Option<&str>` and skips
  sessions whose stored directory canonicalizes to something else
  (`working_dir_in_scope`). The dispatch reads the caller's directory from the
  connection's own agent (`agent.lock().await -> working_dir()`), never the daemon's cwd.
  The check runs on the already-reserved `try_lock_owned` guard: a second `agent.lock().await`
  there would race and report an idle session as busy, silently dropping sessions the user
  asked to continue. An `out_of_scope` count is added to the log line so a suspicious skip is
  visible rather than silent.
- Two deliberate non-filtering cases, documented at the function:
  - When only the caller has a directory, or only the session does, the session stays in
    scope. A directory-less session is attributable to no project, and a caller with no
    directory means the request could not be attributed to one at all, where filtering
    would report "no interrupted sessions" while real sessions sit interrupted.
    This is not a common caller path: `Session::create` populates `working_dir` from the
    process cwd, so a real caller almost always has one.
  - Both sides are canonicalized before comparing, so a symlinked checkout or a `..`
    segment does not read as a different project.
- `recover_headless_sessions_on_startup` is deliberately left daemon-wide. No client asked
  for it, it is the daemon tidying up after its own restart, and narrowing it would strand
  projects with no attached client.
- **Acceptance:** resuming from project A does not deliver a continuation into any live
  session belonging to project B.
- **Test:** `resume_all_skips_sessions_from_other_projects` in
  `crates/jcode-app-core/src/server/client_actions_tests.rs`. Two identically-interrupted
  live sessions rooted in `project-a` and `project-b`, resume-all dispatched with project A
  as the caller. Asserts `resumed == 1`, asserts project B's attachment received nothing
  (`try_recv`, not `recv`: B's attachment stays open for the daemon lifetime so `recv` would
  hang rather than report silence), then sanity-checks A really received its `TextDelta` so
  the test cannot pass by skipping both. With the scope filter disabled the test fails with
  `got ["kikazaru", "iwazaru"]`, confirming it guards the fix rather than the build.
  The two pre-existing resume-all tests pass unchanged.

### P0.3 - Attaching to a live session re-pins that session's working directory

- [x] **Status:** done (commit `0d34e73f3`)
- **Where:** `client_lifecycle.rs` captures the resumer's cwd, `client_session.rs` forwards
  it, `agent/turn_execution.rs:1105-1106` overwrites `session.working_dir` and refreshes the
  context message. (The plan previously cited `server/turn_execution.rs`; the file moved to
  `agent/`.)
- **Existing partial guard:** `subscribe_working_dir_replacement` rejects a report that is the
  home directory when the session already has a different cwd (issue #481). That closes the
  most common case (launching `jcode` from `$HOME`) but not the general cross-project attach.
- **Divergence note:** project-local MCP was resolved from the *subscriber's* raw
  `working_dir_override` while the session's own cwd was separately overwritten. Fixed
  together: both now read one resolved value.
- **Fix:** a client-reported directory is **creation-only**, mirroring the rule the request
  handler already applied to system prompts ("overrides are creation-only; never apply one to
  a target attachment") and which had never been applied to `working_dir`.
  `session_working_dir_for_client` decides it in one place: an existing session directory
  always wins; a session with none adopts the report only while being created; a session that
  stays unattributed stays unattributed rather than falling back to the daemon cwd. The attach
  is **not** rejected: it is the ordinary shape of a client reconnecting, and the target's
  project is preserved instead. A deliberate project move needs an explicit request, which the
  wire protocol does not have.
- **Four writers, one answer:** all four writers reachable from a single attach were reading
  the raw report separately. `handle_resume_session` resolves `bound_working_dir` once (from the
  live target's agent via `try_lock`, else the on-disk copy) and feeds both
  `restore_session_with_working_dir` and `mcp_working_dir`;
  `apply_or_defer_subscribe_working_dir` applies the rule in **both** branches, including the
  deferred `tokio::spawn` one that runs for a busy agent (where a guard applied only to the
  sync path lapses mid-turn); and the swarm re-key is computed after the same rule.
- **Acceptance:** attaching from project A to a session stored under project B leaves B's
  `working_dir`, tools, MCP config, memory scope, and swarm grouping unchanged. **Met.**
- **Test:** `cross_project_attach_preserves_target_session_working_dir` (live target: agent
  directory, post-subscribe directory, swarm member, on-disk copy) and
  `cross_project_attach_of_offline_session_preserves_its_working_dir` (restore-from-disk, the
  only path that binds a directory at all). Each was confirmed to fail with its own guard
  disabled.
- **Behavior change worth noting:** the pre-existing test
  `apply_subscribe_working_dir_keeps_project_when_client_reports_home` asserted that a
  project-to-project move was honored. That assertion *was* the bug, so it now asserts the
  opposite contract, and a session with no directory yet still adopts one at creation.
- **Note:** `Agent::new_with_initial_working_dir(.., None)` does not yield a directory-less
  agent, because `Session::ensure_initial_session_context_message` stamps the daemon process
  cwd when the directory is `None`. Test fixtures that need one must clear it explicitly.

### P0.4 - Session attach has no identity check at all

- [x] **Status:** done
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
- **Decision: option (a), keep it permissive and make the risk explicit.** Option (b) would
  break the session picker's cross-project listing, which is deliberate product behavior
  (see Context above). Option (c) buys nothing here: the socket is local and unauthenticated,
  so a token in the same trust domain adds no isolation, only ceremony. The transport half of
  this item (no auth token, Windows pipe without `SecurityAttributes`) is a separate concern
  and is **not** covered by this decision.
- **Server half (done):** the attach is logged at warn level with the cause it actually
  has. `SubscribeWorkingDirRefusal` distinguishes `HomeDirectory` from `CrossProject`;
  before this, one hardcoded home-directory message was served from all four refusal sites,
  so every cross-project attach was logged as a home-directory report.
  `subscribe_working_dir_refusal_message` builds the text as a value rather than formatting it
  inside the logging call, so the wording itself is assertable rather than only the
  classification.
- **Client half (done):** the client sends its launch directory in `Subscribe` and used to
  discard it, so both values needed to detect the mismatch were present and unused.
  `App.client_launch_working_dir` keeps it, deliberately separate from `session.working_dir`:
  the disagreement between the two is the entire signal. `cross_project_attach_notice`
  compares them the way the daemon does (canonicalized) and names **both** projects.
- **Why a transcript card and not a status notice:** which tree every file and shell tool
  will touch is the most important fact about the session that follows, and
  `TuiState::status_notice` expires after 3 seconds. It is stashed through
  `set_pending_startup_notice` so it survives the remote History bootstrap clearing a fresh
  session's transcript.
- **Where it fires:** remote startup resume (both the normal and the `reload_fast_start`
  variant, which defers the transcript but already has the directory), and an explicit
  `/resume` or workspace switch. The switch path uses
  `note_cross_project_attach_for_session`, which reads the *target's* directory off disk
  because `app.session` still describes the session being left. Guarded by
  `cross_project_attach_notice_shown` so a reconnect does not re-announce.
- **Deliberately quiet:** under SSH, where the process cwd is the laptop's and describes
  nothing about the remote session; and when either directory is unknown. Reporting an
  unverified mismatch would produce a notice on ordinary sessions and train the user to
  ignore it.
- **Acceptance:** met. The permissive attach is documented (here), enforced in the sense of
  being surfaced on both server and client, and covered by tests. Silent permissive attach
  no longer exists.
- **Tests:** `cross_project_attach_notice_names_both_projects`,
  `..._is_quiet_when_the_projects_agree`, `..._stays_quiet_when_either_side_is_unknown`,
  `..._tolerates_unresolvable_paths`,
  `cross_project_attach_shows_the_user_which_project_the_session_belongs_to`,
  `..._notice_fires_once_per_client`, `same_project_attach_shows_no_notice`,
  `subscribe_working_dir_refusal_reason_names_the_actual_cause`,
  `subscribe_working_dir_refusal_log_states_the_actual_cause`.
- **Defect found while writing the tests:** `same_project_dir` fell back to "unequal strings
  means a mismatch" when canonicalization failed, which reported `/repo` and `/repo/.` as two
  projects. The fallback now trims trailing separators first, keeping a bare root intact.

---

## P1: Global state that crosses projects (HIGH)

### P1.1 - MCP shared pool keys entries by server name only

- [x] **Status:** done
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

**Resolution.** Pool entries are keyed by `PoolKey { scope, name, working_dir }`, where
`scope` is the project scope key and identity is `scope` + `name` only. `working_dir` is
carried so the pooled child process is spawned in the right directory, but it is excluded
from `PartialEq`/`Hash` so it can never affect lookup.

The scope key comes from a new shared helper, `crates/jcode-base/src/project_scope.rs`
(`project_key` / `optional_project_key`). AGENTS.md requires one shared derivation point per
concern, and the plan already had two incompatible private copies
(`trim_trailing_separator` in `crates/jcode-tui/src/tui/mod.rs`, unstable `DefaultHasher`
`project_hash` in `crates/jcode-base/src/goal.rs`). The helper canonicalizes, strips the
Windows `\\?\` verbatim prefix, normalizes `UNC\` back to `\\server\share`, trims trailing
separators while preserving a bare root, case-folds on Windows only, and digests with SHA-256.
It deliberately does **not** reuse `goal.rs`'s `DefaultHasher` hash; that instability is
tracked separately as P3.1 and folding it in here would have mixed two backlog items.

`optional_project_key(None)` maps to the literal `"none"` bucket and is never resolved to the
daemon's cwd, per isolation invariant 1. `McpManager` computes its scope from the per-session
`project_dir` on every call, so the pool is scoped by the same answer the rest of the session
uses (invariant 3).

Every unscoped pool entry point gained a `_scoped` sibling rather than being changed in place,
so the single-argument call sites stay readable; `default_scope()` reproduces the previous
behavior for them. `begin_connect` now compares whole `PoolKey`s, which is the actual bug:
it previously returned `Connected` for another project's running process.
`acquire_handles_scoped` filters by scope and `call_tool_scoped` refuses rather than falling
back to another scope's entry. `disconnect_all_scoped` tears down only one project's processes
so a session reload cannot yank another project's servers.

**Tests.** Six new tests in `crates/jcode-base/src/mcp/pool.rs`. The fixture is a stdio MCP
server that reports its own `cwd:pid` through a `whereami` tool, which is what makes
"two distinct processes" observable rather than inferred. Verified by reverting `PoolKey`
identity to name-only: `same_server_name_in_two_projects_gets_two_separate_processes`,
`disconnecting_one_project_leaves_another_projects_process_running`, and
`ref_counts_separate_projects_that_share_a_server_name` all fail, and pass again once
restored. Note that `a_session_cannot_acquire_another_projects_server_handle` passes under the
flattened key too, because the scope filter in `acquire_handles_scoped` independently blocks
that path; it is a guard against regression rather than a proof of this fix.

### P1.2 - `session_search` has no default project scope

- [x] **Status:** done
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

**Resolution.** `resolve_working_dir_filter` in
`crates/jcode-app-core/src/tool/session_search.rs` decides the scope: an explicit argument
wins, `"*"` is the documented opt-out for genuinely global recall, and otherwise the filter
defaults to `ctx.working_dir`. The schema description states both the default and the
opt-out, so the behavior is discoverable from the tool definition rather than only from
source.

When `ctx.working_dir` is `None` the filter stays `None` rather than falling back to the
daemon's cwd. That is the same deliberate `None` decision isolation invariant 1 demands: a
session-scoped path must not resolve from whichever project happened to start the process.
An unscoped search that admits it is unscoped is honest; one silently scoped to the wrong
project is not.

The matcher fallback in `crates/jcode-session-types/src/lib.rs` no longer degrades to
substring matching. A bare filter with no `/` is a project *name* and now matches only whole
path segments, so `jcode` matches `/workspace/jcode` and its subdirectories but no longer
matches `/workspace/jcode-old` or `/workspace/myjcode`. That is the invariant 4 rule about
guarding sloppy-filter fallbacks so they cannot quietly widen the result set.

**Tests.** Four tests in `session_search_tests.rs` and one in `jcode-session-types`. Verified
by reverting each half independently: restoring the substring fallback fails
`bare_working_dir_filter_matches_whole_segments_not_substrings`, dropping the session default
fails `search_defaults_to_the_calling_sessions_own_project`, and removing the `"*"` opt-out
fails `star_is_the_explicit_opt_out_for_cross_project_recall`. All pass again once restored.
- **Test:** write sessions under two temp dirs, call the tool with only a query, assert
  only the current project's sessions come back.

### P1.3 - `bg` background tasks have no ownership enforcement

- [x] **Status:** done
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

**Resolution.** Three independent halves, each with its own regression test.

*Ownership on named task ids.* `assert_task_ownership` (`crates/jcode-app-core/src/tool/bg.rs`)
runs inside `resolve_task_ids`, so it covers both the single `task_id` and the bulk `task_ids`
form for every action that resolves an id. A task whose `session_id` differs from
`ctx.session_id` is rejected with an error naming the task, its owning session, the calling
session, and the opt-out. An id that matches no task is deliberately passed through so the
action itself reports "not found": inventing an ownership error for a typo would send the
caller hunting for a session that does not exist.

*The opt-out is named and real.* `all_sessions` (default `false`) is the single documented
opt-out for crossing sessions. It widens the session filter in `filtered_tasks` as well as
skipping the ownership check. That second part was a real defect found while writing the test:
the flag was documented on `list` and `cleanup`, but `filtered_tasks` only ever read
`session_only`, so `all_sessions=true` silently did nothing to a listing. The schema promises
a listing it did not deliver, which is exactly the undocumented permissive behavior this
plan exists to remove.

*Default scoping.* `list` now defaults to session-scoped, and `cleanup` routes through the new
`BackgroundTaskManager::cleanup_filtered_for_session` unless `all_sessions=true`.
`cleanup_filtered` keeps its global behavior and its doc comment now says so: it is daemon
maintenance, not a tool call. The session filter in `cleanup_filtered_scoped` is applied
*after* the status read, so a task file whose status cannot be parsed is never attributed to
the wrong session and never deleted by another session's cleanup.

**Tests.** Eight new tests, four in `crates/jcode-base/src/background/tests.rs` and four in
`bg.rs`, each proven to fail with its half reverted:
`session_scoped_cleanup_leaves_other_sessions_task_files_alone`,
`global_cleanup_still_sweeps_every_session` and
`session_scoped_cleanup_ignores_files_whose_status_cannot_be_read`;
`cancel_rejects_another_sessions_task`, `task_ids_bulk_is_also_ownership_checked`,
`list_defaults_to_the_calling_session`, `all_sessions_widens_any_session_filtered_lookup`,
and `list_execute_does_not_leak_another_sessions_tasks_by_default`.

One note on how those were verified. The first version of the list test called
`filtered_tasks` directly and it passed even after the call site's default was reverted: a
correct helper is indistinguishable from a call site that stopped using it. The test was
rewritten to drive `BgTool::execute`, writing a uniquely named task file into the real global
task dir, which is the only level that actually observes the dispatch. The same rewrite is
what surfaced the dead `all_sessions` opt-out.

### P1.4 - Schedules are a single global queue with no ownership check

- [x] **Status:** done
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

**Resolution.** Ownership moved to the point of mutation rather than being added to
one caller. `AmbientManager::cancel_schedule` now takes the calling session and returns
`CancelOutcome` (`Removed` / `NotOwned { created_by }` / `NotFound`), so a check that
lives in the tool is a check a new caller can skip. It is the only cancel entry point;
`force_cancel_schedule` is the named escape hatch, documented as usable only by a
caller that has already established cross-session intent.

Ownership is per *creator* session (`created_by_session`), not per target session. The
queue is one file for the whole daemon, and the creator is what ties an item to a
project. Targeting is a separate concern and stays what it was.

`schedule list` filters to the calling session and reports how many items are hidden,
so a scoped list cannot be mistaken for a global one. `schedule cancel` refuses an id
created by another session with an error naming the owning session and the opt-out.
`all_sessions=true` is the single documented opt-out for both actions, matching P1.3's
`bg all_sessions` so the two tools read the same way.

The ambient agent's own cycle schedule (`tool/ambient.rs:213`, `working_dir: None`) is
deliberately global and is unchanged. The TUI ambient widget
(`gather_ambient_info_inner`) still shows every project's queue: that is the user
looking at their own scheduler, not an agent reaching across projects, so it is out of
scope for this item and is recorded here as a deliberate decision rather than an
oversight.

**Tests.** Five, all in `crates/jcode-app-core/src/tool/ambient/tests.rs`, proven to
fail with their half reverted: removing the list filter fails
`schedule_list_hides_other_sessions_schedules_by_default`; disabling the manager's
ownership check fails both `schedule_cancel_rejects_another_sessions_schedule_and_leaves_it_queued`
and `cancel_schedule_refuses_another_sessions_item_at_the_manager`.
`all_sessions_opts_into_cancelling_another_sessions_schedule` and
`a_session_can_still_cancel_its_own_schedule` cover the opt-out and the non-regression
in the allowed direction. They redirect `JCODE_HOME` to a temp dir under
`lock_test_env`, and restore it through a drop guard so a failing assertion cannot leak
a temp home into later tests. 29 `tool::ambient` and 62 `ambient` tests pass.

---

## P2: Daemon-cwd fallbacks (MEDIUM)

`working_dir: None` must never mean "the daemon's directory".

### P2.1 - Project skills overlay falls back to the daemon cwd

- [x] **Status:** done
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

**Resolution.** The decision moved to the point of the read instead of being pushed onto
callers. `load_project_local_dirs` now early-returns when `working_dir` is `None`, so
`None` means "no project overlay" everywhere below it, and `project_local_dir` takes a
non-optional `&Path` so the process cwd can only be reached by a caller that supplies it
deliberately. Two callers genuinely want process scope: the CLI startup memory provider
(`src/cli/startup.rs`) and the matching test (`crates/jcode-base/src/memory_tests.rs`),
because memory retrieval is deliberately process-scoped. Both now pass
`std::env::current_dir()` explicitly, each with a comment saying so, which means the code
matches the comment instead of relying on a fallback the caller never asked for.

`SkillRegistry::load()` was `load_for_working_dir(None)`, had no callers
(`shared_registry`/`shared_snapshot` use `load_global()`), and existed only to provide
that fallback, so it is deleted rather than left as a second entry point with different
scope. The stale doc comments on `load_for_working_dir` and `load_project_overlay` that
promised a process-cwd fallback were corrected in the same change, per the comment-intent
rule.

**Tests.** Two in `crates/jcode-base/src/skill.rs`:
`no_working_dir_loads_no_project_skills_even_under_a_repo_cwd` points the process cwd at a
temp repo that *does* have `.jcode/skills` and asserts that `load_project_overlay(None)`,
`load_for_working_dir(None)` and `effective_for_working_dir(_, None)` all omit it, then
re-asserts with `Some(repo)` as a positive control so it cannot pass vacuously; and
`no_working_dir_skips_every_project_local_skill_convention` covers `.jcode`, `.agents` and
`.claude`. One in `crates/jcode-app-core/src/tool/skill.rs`:
`skill_tool_with_no_working_dir_does_not_list_daemon_cwd_project_skills` drives
`SkillTool::execute` with `ctx.working_dir = None` for both `list` and `load`. That test
binds to the tool call site rather than the helper, because the P1.3 regression showed a
helper test does not prove the call site uses the helper. Reverting the guard and the
permissive `project_local_dir` fails both base tests (`29 passed; 2 failed`) and, with the
tool and its test untouched, the tool test (`14 passed; 1 failed`). 31 `skill::`, 44
`memory::tests` and 15 `tool::skill::` tests pass.

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
- [x] **Cwd binding:** a session created in dir B, attached from dir A, keeps dir B (P0.3).
- [x] **Pool isolation:** same-named shared servers with different configs do not share a
  process (P1.1).
- [x] **Search scoping:** `session_search` with no `working_dir` returns only the current
  project (P1.2).
- [x] **Task ownership:** `bg cancel` from another session is rejected (P1.3).
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
