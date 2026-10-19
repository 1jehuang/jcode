# Idle session memory hibernation

## Design

An open terminal is not evidence of active work. Jcode should release reloadable
memory after a sustained idle period without closing the terminal, losing the
input draft, or interrupting a tool, agent turn, or scheduled wake.

The initial implementation uses a conservative boundary: authoritative session
state stays available, while derived in-memory representations can be discarded.
This is not process suspension, and it does not promise a near-zero-RAM session.

### Configuration

```toml
[session]
hibernation_idle_secs = 1800
```

The default is 30 minutes. Set the value to `0` to disable idle hibernation.
`JCODE_HIBERNATION_IDLE_SECS` overrides the file value. Existing configuration
files without a `[session]` table inherit the default.

### Terminal behavior

The client preserves the last rendered terminal frame and input draft while
releasing derived caches. A local status identifies the hibernated state and
invites the user to type to resume. It is UI state, not a message sent to the
model and not a new conversation turn.

The waking input must be processed normally, including its first character.
Events that change conversation content must wake the renderer as well.
Periodic housekeeping must not repeatedly wake the client and rebuild caches.

### Memory accounting

Memory savings are observed before and after release, not guessed from transcript
length. A TUI RSS change is a measurement for that client process, not the shared
daemon or all sessions. A flat or unavailable reading must not be displayed as a
positive saving. Allocator behavior, shared pages, and other concurrent work can
all affect RSS, so there is no fixed savings guarantee.

The banner names **TUI RSS** explicitly and shows the before/after readings.
Its final sample is taken after the first frozen frame has been rendered, so
the retained screen buffers are included. A positive change is shown as an
increase, not silently reported as a saving.

The daemon measures its own process separately and records
`server_idle_hibernation_before` and `server_idle_hibernation_after` samples in
`server:memory-history`. One sweep purges retained heap once after releasing all
session locks. The daemon's RSS change is not attributed to a particular TUI.

### Released state

- TUI: prepared transcript/body frames, rendered-message lines, syntax
  highlighting, copy viewport, derived side-panel/diff/image state, incremental
  markdown renderer, and the local session's provider-message cache.
- Daemon: idle sessions' provider-message caches and reconstructible tool-output
  indexes. Original tool definitions, captured instructions, undo state,
  provider handles, and canonical messages stay resident.

The terminal keeps a bounded copy of its last frame instead of repainting the
transcript into new caches on every idle tick. The status is cleared on input,
resize, or a meaningful state change. Presence traffic, acknowledgment replies,
and read-only debug queries do not thaw the session.

### Debugging

- `client:hibernation` returns state, before/after RSS, signed RSS change,
  hibernation/wake counts, and the last wake location.
- `client:hibernate` requests immediate hibernation for testing. It bypasses
  the elapsed-time requirement, not active-work guards or the disable setting.
- `client:frame` includes the hibernation status without rebuilding caches.

### Safety invariants

- Active work and pending input must not be discarded or cancelled.
- Session identity, model settings, draft, transcript, and scroll state survive.
- Cache rebuild is transparent and does not invoke resume hooks or append new
  environment snapshots.
- Repeated idle ticks do not repeatedly rebuild and discard the same caches.
- No disk-backed authoritative state may be dropped on the strength of a queued
  save or a save that was skipped because of a migration lease.

## Scope

The first implementation deliberately does not evict the authoritative client
transcript or serialize and destroy the whole agent. Those deeper reductions
need a separate audited persistence/thaw boundary. RAM savings therefore depend
on how much derived state was resident before hibernation, particularly rendered
transcripts and decoded image caches.

## Validation

The implementation was checked with hosted Rust unit tests for config
compatibility, cache ownership/reconstruction, pending-work guards, repeated
server idle cycles, next-turn provider continuity, and TUI input preservation.

An isolated real TUI with a two-second test deadline completed 20 automatic
sleep/wake cycles. The first typed character was preserved at its original
cursor position, local transcript messages survived, and read-only queries did
not reset the idle clock. Its 20 dormant RSS readings stayed within 94,208 bytes.
The visible banner and immediate debug-triggered hibernation were also checked.
An isolated daemon independently ran its real idle monitor and recorded a
single before/after reclamation pair.

Savings depend on workload: a controlled 64 MiB daemon provider-cache fixture
released 66,015,232 bytes of RSS (about 63 MiB), while the small real TUI fixture
released only 278,528 bytes at the populated-cache checkpoint. These are
observations for those fixtures, not promises for ordinary conversations.
