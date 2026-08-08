# Cross-Device Worktree and Build Sync (Proposed)

Status: Proposed (design only, not implemented)

## Problem

A user develops jcode (and other repos) on multiple machines, e.g. a MacBook
(aarch64-darwin) and a Linux laptop. On a single machine, `selfdev build` +
reload means every local jcode instance runs the new version, and multiple
agents can share one worktree because the server mediates edits and tracks
conflicts (`FileTouchService`, `file_activity.rs`).

Split across machines, this breaks:

- A `selfdev build` on the Linux laptop does not update the MacBook's jcode,
  and vice versa.
- There is no shared worktree, so changes made on one machine are invisible
  to agents/sessions on the other until manually pushed/pulled.

Goal: make multiple machines behave like one logical worktree + one logical
build channel, the same way multiple agents already share one worktree on a
single machine.

## Existing building blocks (verified in code)

| Building block | Where | Why it matters |
| --- | --- | --- |
| Arch-independent version identity | `jcode-build-support/src/source_state.rs` (`SourceState::version_label`, `fingerprint`) | Fingerprint hashes full commit hash + status + `diff --binary HEAD` + untracked contents. Identical source trees on two machines produce the same label, even though binaries differ per-arch. |
| Auto-reload on newer binary | `server/util.rs::server_has_newer_binary`, `reload_exec_target` | Mtime-based channel scan. A peer-triggered local build that publishes to `builds/current` triggers the existing reload flow with zero changes. |
| Pull → build → install → exec | `session_rebuild.rs` | Already implements the receiving side's pipeline shape. |
| Network door, same protocol | `jcode-base/src/gateway.rs` (WS + plain HTTP on :7643) | Remote clients speak the identical newline-JSON protocol as Unix-socket clients. Plain HTTP handler (`/pair`, `/health`) is a natural place for `/peer/*` endpoints. |
| NAT-friendly device event bus | `server/cloud_relay.rs` | Device IDs, heartbeats, long-polled command events. Works when machines cannot reach each other directly. |
| Server-side tool execution | server architecture | Tools (bash, edit) run in the server process; a remote client attaching to another machine's server gets the full multi-agent-one-worktree behavior, including conflict warnings. |
| Remote build precedent | `scripts/remote_build.sh` | rsync + ssh + sync-back pattern. |

### Known gap: repo identity across machines

`repo_scope_key` / `worktree_scope_key` hash the *local canonical path* of the
git common dir / worktree. These will never match across machines
(`/Users/jeremy/...` vs `/home/jeremy/...`). Cross-device features must key
repos by something portable: normalized origin URL, or an explicit repo name
in config (`[sync] repo_id = "jcode"`), falling back to origin URL hash.

## Design tensions

1. **Binaries cannot be shared** across darwin-aarch64 and linux-x86_64.
   "Same version everywhere" must mean *same source state, built per machine*,
   with `version_label` as the cross-machine equality check.
2. **Not all writes are server-mediated.** Tool edits flow through the server,
   but `bash`, editors, and `cargo` mutate the worktree invisibly. Cross-device
   sync therefore needs a byte-level capture mechanism (git plumbing snapshot
   and/or fs watcher), not just tool-event forwarding.
3. **Laptops go offline.** Pure live-sync (mutagen/syncthing style) has poor
   conflict semantics. Git-based convergence (per-device sync refs, merges)
   handles offline divergence honestly.

## Mechanism: shipping a worktree state without touching HEAD

To capture a possibly-dirty worktree atomically without moving the user's
HEAD or index:

```
GIT_INDEX_FILE=$tmp git add -A          # tracked + untracked into temp index
tree=$(GIT_INDEX_FILE=$tmp git write-tree)
commit=$(git commit-tree $tree -p HEAD -m "jcode sync: <device> <fingerprint>")
git push origin $commit:refs/jcode/sync/<device>
```

- Atomic, content-addressed, includes untracked files, excludes gitignored.
- Receiver fetches the ref, applies it (checkout into worktree or materialize
  diff), then verifies `current_source_state().fingerprint` matches the
  beacon's fingerprint, guaranteeing byte-exact reproduction.
- The sender's worktree/index/HEAD never move.

## Phased plan

### Phase B - selfdev build parity (do first; kills the stated pain)

After a successful `selfdev build` + publish on machine X:

1. Compute `SourceState` (already done by the build pipeline).
2. Snapshot the worktree to `refs/jcode/sync/<device>` (mechanism above) and
   push to the shared git remote. Clean trees can skip the snapshot and use
   the existing commit.
3. Announce a **version beacon** `{repo_id, version_label, fingerprint,
   full_hash, sync_ref, device, timestamp}`:
   - Fast path: HTTP POST to peer gateways (`/peer/version-beacon`) over
     Tailscale.
   - Fallback: cloud relay device event (works through NAT).
   - Slow path: peer polls sync refs on the git remote.

Machine Y's server runs a small peer-sync task (same shape as
`cloud_relay::spawn_if_configured`):

1. Receives beacon; ignores if `version_label` matches what it already runs
   or recently applied (**echo suppression**, prevents rebuild ping-pong).
2. Policy gate, default conservative:
   - Worktree clean AND local HEAD is an ancestor of the beacon commit
     → auto-apply: fetch, advance, build via the selfdev build queue
     (native arch), publish. The existing `server_has_newer_binary()` poll
     then auto-reloads.
   - Otherwise → do not clobber. Surface in TUI/status:
     `peer build available: <label> from <device> (blocked: local changes)`
     with a one-keystroke accept.
3. After publish, Y's `version_label` equals X's. Cross-device parity is
   verifiable by comparing labels (e.g. in `selfdev status` and the beacon
   acks).

Config sketch:

```toml
[sync]
enabled = true
repo_id = "jcode"                  # portable repo identity
peers = ["macbook.tail-net.ts.net:7643"]
auto_apply = "clean-ff-only"       # off | clean-ff-only | always-notify
```

### Phase A - hub attach (one authoritative worktree when both online)

`jcode attach <host>`: TUI connects to the peer machine's server through the
existing gateway WS. Because tools execute server-side, the attached client
participates fully in that machine's worktree, conflict tracking included.

Work items:

- Client transport: WS stream in place of Unix socket (bridge already exists
  server-side; needs a client-side counterpart).
- Pairing/auth UX for a trusted personal device (DeviceRegistry exists).
- Audit client-side local-disk reads that assume the session's filesystem,
  e.g. `jcode-tui/src/tui/ui_file_diff.rs:270` (`std::fs::read_to_string` of
  the diffed file). These need server RPCs (a `read_file` control request) or
  graceful degradation.

### Phase C - true worktree federation (long-term)

Generalize Phase B's snapshot + Phase A's peering into continuous two-way
sync:

- **Data plane:** throttled auto-snapshots to per-device sync refs, triggered
  by (a) server-mediated tool edits, (b) an fs watcher for bash/editor/cargo
  writes, (c) timers. Peers fetch refs directly (ssh/Tailscale) or via the
  shared remote.
- **Convergence:** if local HEAD/state is an ancestor → fast-forward apply.
  If diverged → keep both snapshots, mark the repo "split", and let the
  harness spawn an agent to perform the merge (the cross-device analog of
  the server managing same-machine conflicts).
- **Coordination plane:** federate `FileTouchService` events over the peer
  link so agents on both machines see "another agent edited lines 40-60"
  warnings across devices. Optionally add advisory write leases for hot
  files.

## Alternatives considered

- **Syncthing/mutagen for the worktree:** simple, but byte-level conflicts
  (`.sync-conflict` files), no atomicity across multi-file edits, and target
  dirs / build artifacts need careful exclusion. Git-plumbing snapshots give
  atomic, content-addressed, mergeable states using semantics git users
  already understand.
- **Always build on one machine + copy binaries:** broken by arch mismatch;
  cross-compiling darwin from linux (and vice versa) is not worth the
  toolchain cost given both machines have working local toolchains.
- **NFS/SSHFS shared worktree:** punishes offline use and IDE/file-watcher
  performance; a non-starter for laptops.

## Suggested order of implementation

1. Phase B beacon + receiver with `auto_apply = "clean-ff-only"`, git-remote
   polling only (no new ports), `selfdev status` showing peer parity.
2. Add gateway `/peer/version-beacon` fast path + TUI notification for the
   blocked case.
3. Phase A `jcode attach` (WS client transport + pairing + file-read RPC).
4. Phase C federation, reusing the beacon snapshot machinery and the peer
   link from A.
