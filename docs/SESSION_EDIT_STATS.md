# Per-session edit statistics

Harness API v1.4 adds optional `SessionInfo.edit_stats`:

```json
{"added": 120, "removed": 35, "approximate": false}
```

These are **cumulative changed lines**, not a net worktree diff. Replacing a line
counts one addition and one removal. Editing it again counts again. Built-in
`write`, `edit`, `multiedit`, `patch`, and `apply_patch` mutations are covered,
including successful subcalls inside `batch`. Shell commands, MCP tools, and
external editors are not covered. No repository-wide git state is consulted.

## New tool executions

Each successful filesystem mutation records a full before/after line diff,
independent of truncated tool previews. No-op edits add zero. Failed/proposed
edits add nothing. If a multi-file tool partially succeeds and later fails,
mutations already performed still count. Ordinary moves count content changes
rather than treating an unchanged file as deleted and recreated.

Small counters live at `$JCODE_HOME/sessions/edit-stats/<session-id>.json`
(default `~/.jcode`). A per-session OS file lock serializes concurrent updates,
and atomic replacement prevents readers from observing partial JSON. Counters
survive transcript compaction and are separate for agents sharing a worktree.
The first counter seeds a best-effort estimate from older persisted history.
Forked legacy history is not attributed to the child.

Limits: file mutation and counter persistence are not one transaction. A crash
or disk error between them can lose accounting. Concurrent external writes to
the same file can race the tool's before-content read. Unreadable old text and
legacy migration are marked approximate. This is not an audit log.

## Existing running daemons and history

The Rust SDK exports `SessionEditStats` and
`enrich_sessions_from_local_edit_stats(&mut [SessionInfo])`. Invoke the helper
on **fresh local** session-list responses during periodic refreshes. It honors
nonempty API values. Never apply it to a remote daemon's sessions.

Exact sidecars are available immediately. For older daemons, the helper queues
one background reader, caches by snapshot/journal size and modification time,
and returns completed estimates on later refreshes. The queue and cache each
hold up to 512 sessions. Each legacy snapshot plus journal is capped at 32 MiB.
Missing, malformed, oversized, and forked legacy records remain unavailable.
Truncated output, compaction, aliases in old batch headers, and unsupported
legacy formats may omit edits. Display `approximate: true` as an estimate, not a
strict lower bound. Old history cannot generally be reconstructed exactly.

A running daemon does not need to be restarted for the SDK's legacy estimates.
New exact mutation recording requires a daemon running the new implementation.
