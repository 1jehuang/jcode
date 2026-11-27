# Server session runtime ownership

The server enforces three separate invariants. A connection is an observer and
request source, not the owner of an independent copy of a resumed session.

## One published runtime per session

`server::SessionManager` owns the private live-runtime map. Production callers
have read access, but no mutable-map accessor. Creation and restoration acquire
a per-session `SessionReservation` before looking up or constructing a runtime.
Headed resume, startup headless recovery, and lazy worker revival share this
publication authority.

Publication rejects a different existing owner and validates the Agent's session
ID. Removing an entry does not authorize another writer while an old task or
connection still retains the runtime. Weak ownership tombstones enforce that
rule without retaining idle Agents. Rekeying checks both entries before changing
the registry. A stale migrated transcript refreshes the canonical idle runtime in
place rather than replacing its Arc.

Cold resume and clear construct a fresh runtime, not a replacement inside a
published source Arc. The cold restore API consumes an owned `Agent`, not a
shared handle, and returns its restored handle only after restoration succeeds.
A connection leaving a shared session cannot silently
change the session underneath other windows. The old runtime and its control
metadata are retired only when the connection registry confirms no remaining
attachments. The fresh runtime remains reserved through publication bookkeeping.
Mutation reservations share the execution gate but do not allocate an active
turn or advance its watermark, so reconnect snapshots never report bookkeeping
as model work that has no eventual terminal event.

Reservations are cancellation-safe and scoped to one manager and session ID.
Acquire them before connection, registry, or Agent guards. Release them before
history delivery or other potentially slow client I/O.

The manager also publishes a read-only turn observer. History bootstrap can read
runtime identity while a provider turn holds the Agent mutex.

## One accepted turn per runtime

An Agent-owned, noncloneable execution lease is acquired at acceptance, not only
when the spawned task eventually locks the Agent. Public execution entrypoints
acquire a lease. Reserved inner paths validate that their lease belongs to the
same runtime, allowing failover and followups without reentrant acquisition.

The lease includes the publication lifetime. A scoped fanout queue retains a
publication pin until already-enqueued events drain, including when the producer
is aborted. Closing the queue rejects late writes through retained sender clones.
A newer turn cannot begin while an earlier turn can still publish queued output.
Connection-local task completion also checks task identity, so a reused request
ID cannot complete a different task.

## Stale events cannot complete newer UI work

Identity-aware clients negotiate support. Turn identities contain a runtime UUID
and a monotonic runtime-local turn number. Scoped turn events and authoritative
history bootstrap allow the TUI to reject events from an obsolete runtime or
turn. Legacy clients continue receiving unwrapped events and do not receive new
identity-only frames. Legacy clients do not gain stale-event fencing.

These are server-process invariants, not a distributed storage lock. A separate
server process or external program writing the same session file is outside this
manager's authority. Migration leases and existing process coordination still
apply. Test-only mutable registry helpers are explicit fixture escape hatches,
not production publication APIs.

## Regression coverage

- Manager tests cover concurrent factory execution, failure and cancellation,
  retained retired owners, reservation isolation, atomic rejected rekeys,
  mismatched session IDs, and observer access while an Agent is locked.
- Resume tests cover two cold attaches sharing the exact same runtime Arc and
  same-session reattachment preserving runtime ownership. Socket fork/restore
  and clear regressions verify shared source observers keep their original owner.
- Turn tests cover exclusive runtime capabilities, foreign lease rejection,
  abort release, queue-drain ownership, and late sender rejection.
- Wire/TUI tests cover stale turn and runtime events, reconnect bootstrap, and
  negotiated legacy compatibility.

A successful build alone is not runtime validation. When testing a rebuilt
server, use an isolated socket or the coordinated self-dev reload path so the
long-lived daemon is actually running the changed binary.
