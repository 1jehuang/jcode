# Remote Handoff (design sketch)

Status: exploration. Nothing here is implemented yet.

## The idea

A session is currently pinned to the machine whose `jcode` server owns it. Remote
handoff means: **move a live session, mid-turn if needed, from one host to another**,
without losing transcript, tool state, or the user's attention.

Three motivating stories:

1. **Laptop → desktop.** Battery is dying, the build needs 8 cores. `/handoff desktop`
   moves the session to `arch-linux-desktop` and the local TUI reattaches over SSH.
2. **Desktop → laptop.** Walking away from the desk; the overnight run should keep
   executing on the desktop but I want to watch and steer from the laptop.
3. **Any host → phone/web.** Same session, thin client, via the relay.

These are actually two distinct primitives that people conflate:

- **Migration**: ownership of the session moves hosts (workspace and processes move too).
- **Attach**: ownership stays put, a remote client drives it (this is closer to what
  `allow_session_takeover` already does, just over a non-local transport).

Most of the value is in attach. Migration is the hard, rarer one.

## What already exists

| Piece | Where | Notes |
| --- | --- | --- |
| Session takeover between clients | `server/client_session.rs` (`allow_session_takeover`) | Handles owner conflict, client instance identity, local-history heuristics. Already the right decision surface. |
| SSH ControlMaster profiles | `app-core/src/ssh_remote.rs` | Named hosts, verified background control socket, headless reuse. |
| Unix socket protocol | `server/socket.rs`, `client_api.rs` | Line-delimited JSON `Request`/`ServerEvent`. Transport-agnostic in shape, not in code. |
| Reload handoff | `server/reload.rs`, `restart_snapshot.rs` | Already serializes live server state across a process swap. This is migration, minus the network. |
| Relay | `server/jade_relay.rs` | Long-poll bridge to a remote control plane; the phone/web path. |
| Harness API | `jcode-harness-api{,-server}` | A second, more structured client surface. |

The important observation: **reload already solves the state-transfer half of
migration**, and **takeover already solves the ownership half of attach**. Remote
handoff is mostly plumbing those two through a transport that is not a local socket.

## Design

### Layer 0: transport abstraction

Today clients dial `socket_path()`. Introduce a `SessionTransport` with three impls:

- `Local(UnixStream)` — today's path, zero behavior change.
- `Ssh(profile)` — `ssh -S <control-socket> <target> jcode serve --stdio`, framed over
  stdin/stdout. Reuses the existing verified ControlMaster, so no new auth surface and
  no credential handling in jcode.
- `Relay` — existing jade relay framing, for hosts that cannot be SSH'd into.

Everything above this layer keeps speaking the same `Request`/`ServerEvent` JSON. This
is the single change that makes the rest cheap.

### Layer 1: remote attach

`/attach desktop` or picking a remote host in the session picker:

1. Resolve the SSH profile, ensure the control master is alive
   (`is_control_master_alive`, else `spawn_control_master_terminal`).
2. Open a transport, `Subscribe { target_session_id, allow_session_takeover: true, .. }`.
3. Remote server runs the existing takeover decision. Same conflict logging, same
   rejection cases. A remote client is just another client instance.
4. Transcript backfill uses the existing `client_has_local_history` path so we do not
   resend the whole session over a slow link.

Failure mode to design for explicitly: link drops mid-turn. The remote server should
keep executing the turn (it already does when a client disconnects) and the client
should reconnect and replay from the last received event id. That requires event ids to
be monotonic per session, which they are not clearly guaranteed to be today. Worth
fixing regardless of handoff.

### Layer 2: migration

`/handoff desktop --move`. Sequence:

1. **Preflight** on the target: jcode present and version-compatible, workspace path
   exists, git remote/commit matches, provider credentials available. Refuse loudly
   rather than half-migrating. Version skew is the most likely real-world failure.
2. **Quiesce**: finish or checkpoint the current turn. Reuse the graceful-shutdown
   tool-handoff logic in `turn_streaming_mpsc.rs` (`allow_reload_handoff`) which already
   knows how to interrupt a bash tool and record a resumable result.
3. **Snapshot**: reuse `restart_snapshot` to serialize session state, then ship
   transcript + snapshot + pending queue over the transport.
4. **Adopt** on the target, verify it can render the session, then **retire** the source
   into a tombstone that redirects any client that reattaches locally.
5. Source keeps a read-only copy for N days. Never delete on migrate.

Non-goals for v1: moving running background bash processes, open file handles, or
uncommitted worktree state. Uncommitted changes should be handled by *committing*
(the repo already prefers commit-as-you-go) or by an explicit rsync step with a diff
preview, not silently.

### Ownership model

Exactly one host owns a session at a time. A lease with a heartbeat, written into the
session record, is enough:

- Owner renews every few seconds.
- A would-be owner may steal a lease only after it expires, or with an explicit
  `--force` that logs the steal.
- Split-brain is the thing to avoid; two servers appending to one transcript is much
  worse than a brief refusal.

## UX

- `/remote add desktop` — existing ssh profile flow.
- `/remote list` — hosts, reachability, sessions each owns.
- `/attach desktop:<session>` — drive a remote session from here.
- `/handoff desktop` — move this session there, then reattach to it remotely so the
  user's view never goes blank. This is the key detail: handoff should *look* like
  nothing happened except a status line change.
- Header shows `● desktop` when the session is not local, with round-trip latency.

## Open questions

1. Do remote tool permissions prompt on the client or resolve on the host? I think the
   host owns the policy, the client only renders the prompt. Otherwise a compromised
   client escalates.
2. Latency: every keystroke-adjacent interaction over SSH is fine (~ms on LAN, tolerable
   over WAN), but the streaming render path assumes cheap event delivery. Coalescing may
   be needed at the transport.
3. Does the swarm coordinator span hosts? Natural extension (spawn workers on the beefy
   box), but it multiplies the ownership problem. Later.
4. Trust boundary: SSH gives us authn/authz for free. The relay does not, and needs a
   real story before it carries full session control.

## Suggested first slice

Layer 0 + Layer 1 attach only, SSH transport, no migration. That is genuinely useful on
its own (drive the desktop from the laptop), reuses the existing takeover logic, and
forces the transport abstraction and the reconnect/replay fix which everything else
depends on.
