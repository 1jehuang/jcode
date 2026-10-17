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
