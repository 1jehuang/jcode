# Concurrency telemetry v2

## Ownership and scope

`begin_concurrency_session(session_id, parent_session_id)` returns a non-cloneable
`ConcurrencySession` guard for one live runtime Agent incarnation. The Agent
holds it, including while idle or running in the background. TUI viewers must
not create guards. The process-global `begin_session` API does not create one.
Call `finish()` when an Agent closes, crashes, clears, or changes session identity,
even if its object remains in a server map. Drop releases ownership as a fallback.
An inert opt-out guard retains `session_id()` but returns false from `is_active()`.

The metric scope is `runtime_agent_sessions`. It counts participating runtime
incarnations sharing the same local `JCODE_HOME`, including multiple Agents in
one daemon and Agents in other processes. It does not count saved session files,
connected viewers, old clients, opted-out Agents, or sessions on other machines.
Two independently running incarnations of the same persisted session count twice.

`root` means no parent ID. `child` means a parent ID is present, including both
manual split/transfer sessions and automated agents. This is not a reliable
interactive-versus-autonomous-worker classification.

## Event contract

The dedicated event is `session_concurrency`, with `phase` equal to `start` or
`end`. It includes the standard telemetry envelope, installation `id`, logical
`session_id`, a random `concurrency_session_id` for pairing this incarnation,
`agent_role` (`root` or `child`), `concurrency_tracking_version: 2`,
`concurrency_tracking_scope: "runtime_agent_sessions"`, and boolean
`concurrency_tracking_available`.

Available events include:

| Field | Meaning |
| --- | --- |
| `active_sessions_at_start` | All live participants at this registration, including self |
| `other_active_sessions_at_start` | Start total minus one |
| `root_sessions_at_start` | Root participants at registration |
| `child_sessions_at_start` | Parent-linked participants at registration |
| `max_concurrent_sessions` | Highest total over this incarnation's observed lease transitions |
| `max_concurrent_root_sessions` | Independent highest root count over that interval |
| `max_concurrent_child_sessions` | Independent highest child count over that interval |
| `multi_sessioned` | Whether total peak exceeds one |

Start-event peaks equal start counts. End-event peaks include joins by other
Agents, even when this Agent did nothing during their entire lifetimes. Root and
child start counts sum to the total, but independent lifetime category peaks need
not sum to the total peak. Unavailable events omit all eight numeric/boolean
measurement fields rather than substituting zeros.

Existing `session_start`, `session_end`, and `session_crash` payloads also carry
version 2, but use scope `legacy_process_global`, availability false, and omit old
concurrency fields. Their singleton accumulator cannot identify logical Agent
ownership. They are not the source for corrected concurrency analysis.

## Locking and failure behavior

All lease names are random UUIDs, never caller-supplied session IDs. Each guard
holds an exclusive OS file lock in `telemetry_concurrency_v2`. Locks disappear on
process death. A registry lock serializes publication and removal so simultaneous
starts cannot both claim the same preexisting count. Successful joins atomically
replace one registry snapshot containing every live owner's high-water marks.
There is no mtime expiration, heartbeat requirement, or PID-reuse heuristic.

An unlocked marker with a published record is stale and is pruned without being
counted. A live or stale *unpublished* lease indicates an interrupted or failed
registration. Such evidence is retained and overlapping owners emit unavailable
measurements until all affected owners have exited. This also handles a failed
short-lived child that exits before an idle survivor's final observation. New
owners during that degraded interval remain unavailable. A quiet point allows
recovery, including from a corrupt old registry snapshot.

Limitations:

- OS locking and atomic replacement must work on the shared local filesystem.
- Failures before a lease can be created cannot communicate missing coverage to
  peers. This is participating-owner telemetry, not a universal Agent inventory.
- Hard kills racing a liveness scan are observed at the individual OS lock probes.
  There is no globally atomic snapshot of process death across the operating system.
- Files are close-on-exec. A fork-only child can retain an inherited lease after
  the owner's hard kill until it execs or exits. Clean finish explicitly unlocks
  the lease, so inherited descriptors cannot extend a clean ownership interval.
- Atomic registry writes protect against interrupted publication, not disk loss
  or power-loss durability. Registry lock acquisition waits at most two seconds.
- Start and Drop-end events use the existing best-effort background queue. An
  explicit `finish()` attempts bounded blocking delivery (800 ms), but neither
  delivery mode is a durable spool or an exactly-once guarantee. Crashes can leave
  unmatched starts and dropped events. Never count unclosed backend starts as
  live sessions. Use available reported measurements instead.
- Opting out suppresses subsequent events. Explicit finish/Drop still releases
  an already-held lease, without requiring telemetry delivery to succeed.

## Verification

Run `cargo test -p jcode-telemetry-core --lib`. Tests cover multiple owners in one
process, synchronized cross-process start races, hard-killed owners and registry
writers, ancient live leases, fresh stale markers, idle-owner peaks, independent
role peaks, degraded intervals and recovery, payload/version invariants, opt-out,
path traversal inputs, and concurrent first-install identity creation.
