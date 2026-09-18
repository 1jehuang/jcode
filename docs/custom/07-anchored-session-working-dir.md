# [07] Session project directory is anchored at creation and immutable

## Requirement

A session's `working_dir` is its project identity: memory buckets, goals,
AGENTS.md snapshots, env snapshots, swarm grouping, and project-local MCP
config all key off it. Before this patch, a remote client that attached
(`subscribe`/`resume`) from a different directory overwrote
`Session.working_dir` with its own cwd — silently re-bucketing the
session's project memory and pointing MCP discovery at the wrong tree.
The existing home-dir guard (issue #481) only rejected reports equal to
`$HOME`; any other divergent cwd still re-pinned the session.

Observed symptom: a session created in project A, resumed over SSH from a
client sitting in directory B, had its memory bucket moved to B — project
memories written under A became invisible.

An intermediate revision of this patch recorded divergent reports in a
`client_working_dir` field; the field had no consumer and the user asked
for strict semantics instead: the anchor is written once and nothing may
rewrite it, so divergent reports are dropped entirely rather than stored.

## Behavior

- `Session.working_dir` is the **anchored project directory**. It is
  bound once — at session creation (new/fork/split/crash-recovery/
  ambient/comm/import session constructors all pass the parent's or the
  requested dir) or, for sessions that somehow reach first contact
  unanchored (legacy persisted files), at the first subscribe/restore —
  and is **immutable afterwards by any mechanism**: subscribe reports,
  resume reports, MCP attach, `/clear`, nothing.
- `Session::anchor_working_dir_if_unset(&str)` is the single entry point
  for client-reported dirs:
  - empty/blank report → no-op;
  - session has no anchor → report becomes the anchor, returns true;
  - session already anchored → report dropped, returns false (a report
    equal to the anchor is just a no-op drop).
- Anchoring an unbound session refreshes the AGENTS.md snapshot, the
  initial session-context message, and logs an env snapshot
  (`Agent::anchor_working_dir_if_unset`, env label `working_dir_anchor`).
- A divergent report is logged at debug level
  ("keeps anchored working dir …; dropped divergent client cwd …").
- Subscribe path: `apply_or_defer_subscribe_working_dir` calls
  `apply_subscribe_working_dir` (immediate or deferred under the agent
  lock). `effective_subscribe_working_dir` resolves anchor-first.
- Project-local MCP resolution (`mcp_working_dir`) prefers the anchored
  dir over the subscribe report in both `handle_subscribe` and
  `handle_resume_session`; the report is only a fallback for unanchored
  sessions.
- `restore_session_with_working_dir` (remote resume) routes through
  `anchor_working_dir_if_unset` instead of overwriting. `/clear`
  constructs the replacement agent with the preserved anchor; no client
  dir is carried.
- `Agent::set_working_dir` is now `#[cfg(test)]` — production has no
  post-construction rewrite path left.
- The client-side SSH placeholder session still clears its stub's
  `working_dir` in `tui_lifecycle.rs` so a stale local path is not
  mistaken for the remote project; that write only touches the
  client-local stub before first subscribe, never the server anchor.

## Config surface

None. Behavior change only. The removed `client_working_dir` serde key is
ignored on deserialize, so old session files/journals load cleanly.

## Affected files

- `crates/jcode-base/src/session.rs` — dropped `client_working_dir`,
  `anchor_working_dir_if_unset`, plumbing through `SessionStartupStub`,
  `RemoteStartupSessionSnapshot`, `session_from_*`, `apply_journal_meta`,
  `journal_meta`.
- `crates/jcode-base/src/session/journal.rs`,
  `crates/jcode-base/src/session/crash.rs` — dropped journal-meta field
  and crash-recovery copy.
- `crates/jcode-app-core/src/agent.rs` — `Agent::anchor_working_dir_if_unset`;
  deleted `client_working_dir()` getter and
  `set_working_dir_for_pending_context`.
- `crates/jcode-app-core/src/agent/provider.rs` — `set_working_dir`
  gated to `#[cfg(test)]`.
- `crates/jcode-app-core/src/agent/turn_execution.rs` — restore path and
  `/clear` no longer carry a client dir.
- `crates/jcode-app-core/src/ambient/runner.rs` — dropped redundant
  post-construction pin (child session already anchors at construction).
- `crates/jcode-app-core/src/server/client_session.rs` — subscribe
  rewrite (`apply_subscribe_working_dir`, anchor-first
  `effective_subscribe_working_dir`, deleted
  `subscribe_working_dir_replacement`), `/clear`, swarm `bound_dir`,
  `mcp_working_dir` sites.
- Tests: `session_tests/cases.rs` (`anchor_working_dir_if_unset_*`),
  `client_session_tests.rs`, `agent_tests.rs`, `comm_session_tests.rs`,
  `desktop_selfdev.rs`.

## Upstream status

General fix, upstream candidate. The divergent-cwd re-bucketing bug is
not fork-specific — upstream's subscribe path still prefers the client
report (including for MCP resolution, which this patch also corrects to
anchor-first). If upstream lands an equivalent fix with its own field
naming, this patch collapses to the strict no-`client_working_dir`
semantics.

Note: while landing this, several `agent_tests` needed explicit titles
because upstream `783c979a0` ("avoid persisting untouched sessions") now
skips writing title-less sessions, breaking save→restore test fixtures.
The added titles are test scaffolding, not a behavioral fix.
