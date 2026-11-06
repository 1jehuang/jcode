# Managed cloud automatic updates

Status: proposed design, not deployed. Source inspection: November 6, 2026.

## Decision

Automatically update managed Jcode Cloud hosts by default, using a centrally
approved stable release and safe daemon activation. Do not enable the ordinary
CLI updater on these hosts and do not restart the fleet immediately on every
GitHub release. Updates must not depend on a user opening a local client.

## Current implementation

The managed service is in the separate `solosystems-managed-cloud` repository:
`workers/api/src/cloud_hosts.js`.

- `CLOUD_JCODE_VERSION` and `CLOUD_JCODE_SHA256` currently pin bootstrap to
  `v0.88.0`. Provisioning downloads that exact archive, checks its checksum,
  and installs `/opt/jcode-cloud/jcode`.
- The user CLI wrapper and systemd daemon both set `JCODE_NO_UPDATE=1`.
  The daemon runs through `jcode-cloud-serve` with `Restart=always`.
- Waking a stopped VM calls EC2 `StartInstances`. The readiness unit waits for
  the daemon socket. This path does not select a new runtime release.
- The current VM idle shutdown checks SSH connections, not agent activity.
  That is not a sufficient update-safety signal, especially for detached work.

The local client's `src/cli/cloud_managed.rs` provides another upgrade path:

- A release client selects its own exact release, or the local
  `JCODE_CLOUD_RELEASE` override. It retains a newer capable installation.
- Development clients normally accept an already-capable runtime. When an
  upgrade is required without a selected tag, the script resolves GitHub's
  latest release. This is not a fleet update policy.
- Preflight verifies checksums, archive layout, exact candidate version, and
  required cloud command capabilities. It atomically replaces the backend,
  preserves launchers, and rolls back failed post-switch verification.
- It deliberately does not restart an existing daemon. Installed CLI version
  and running daemon version can therefore differ.

The daemon has reload machinery (`crates/jcode-app-core/src/server/`), but
`client_session.rs::handle_reload` is not an idle-only activation gate. It can
queue a reload when the agent lock is busy. Do not treat ordinary reload or
absence of SSH clients as proof that automatic activation is safe.

These are findings from local source checkouts, not an audit of running VMs.

## Proposed behavior

1. **One managed stable channel.** Release automation nominates a complete,
   verified stable release into a managed-cloud manifest. Include an immutable
   release ID, platform artifact, digest, rollout cohort, and manifest generation.
   Hosts accept only the configured trusted HTTPS control plane and artifact
   origins. Preview and development builds never enter this channel implicitly.
2. **Automatic checks.** Install a systemd updater timer on each managed host.
   Check at boot and roughly every 15 minutes with jitter. Failed discovery or
   download leaves the last good version usable. Do not wake stopped VMs merely
   to update them or charge compute for fleet maintenance.
3. **Stage, then activate.** Download into a root-owned immutable release
   directory. Verify digest, safe archive structure, executable identity,
   bundled libraries, and cloud protocol capabilities before publication.
   Serialize updater and client preflight with one host-wide install lock.
4. **Safe activation.** Add a daemon-owned atomic idle-update operation. It must
   check every session, swarm worker, active tool, queued turn, session transfer,
   and background task that cannot survive exec. Claim the activation boundary
   under the same admission control that starts work, persist session state,
   and reconnect clients through the existing reload lifecycle. A busy host
   reports `pending`, continues work, and retries later. Never impose a deadline
   that kills active work. Until this gate is implemented and tested, stage only
   and activate at the next normal daemon start.
5. **Target the managed artifact explicitly.** Ensure activation executes the
   approved `/opt/jcode-cloud` candidate, not an unrelated local stable or
   self-development binary selected by general reload discovery. Keep both the
   CLI launcher and daemon launcher environment intact.
6. **Health and rollback.** Check the actual running daemon's version and API
   readiness after activation, not just socket existence or CLI `--version`.
   Retain the previous release. Restore it on startup or health failure using a
   bounded supervisor recovery path. Persist a failed-release quarantine so
   `Restart=always` and the update timer cannot create a crash loop. Any rollback
   of a running daemon follows the same work-safety rule. Release promotion must
   require backward-compatible persisted state, or explicitly disable automatic
   rollback for that migration and withhold rollout until a safe plan exists.
7. **Gradual promotion.** Start with internal hosts, then a small deterministic
   cohort, then the fleet. Advance only after a defined observation window and
   adequate successful activations. Pause on failed health checks, crash loops,
   or protocol failures. A withdrawn manifest stops further installations.
   Do not claim inference error-rate rollback until that telemetry exists.
8. **User visibility and control.** Expose running, installed, target, pending,
   last-check, and failure state through cloud status. Default to automatic
   stable updates. Add a persistent exact-version pin and operator pause.
   A pin is distinct from today's `JCODE_CLOUD_RELEASE`, which only chooses a
   client preflight download target and does not force a downgrade.

## Local self-development and client-initiated updates

Support this explicitly. A local development client can be ahead of every
published release. Stable fleet updates must not overwrite a selected dev build.
Use three persistent per-host policies:

- `stable` (default): follow the approved managed stable channel automatically.
- `pinned`: keep an exact release until the owner changes the policy.
- `dev`: use an owner-uploaded artifact identified by content digest and build
  metadata. Suspend stable activation until the owner explicitly returns to
  stable, including across VM sleep and wake.

Add an explicit client-initiated workflow, conceptually `jcode cloud runtime
push`, plus `status` and `use stable`. These are proposed commands, not existing
interfaces. Offer a push if the local dev client requires capabilities missing
from the cloud daemon. Do not automatically upload a binary on every connection
or propagate every local self-dev reload to the cloud. Compatible older cloud
builds can remain in use even when the local client is newer.

Client pushes must use authenticated transport and the same install lock,
verification, safe activation, health checks, rollback, and quarantine as stable
updates. Record desired policy separately from the running artifact. Publish
policy and verified artifact selection transactionally. Failed staging preserves
the previous selection. Returning to stable can deliberately downgrade a dev
build, so it needs an explicit owner request and state-compatibility checks,
not ordinary automatic version comparison.

Require a package for the cloud host's actual OS, architecture, and libc, with
its bundled libraries. A macOS or ARM laptop executable cannot run on the current
Linux x86_64 host. Use an explicitly selected compatible artifact or a
user-authorized target build. Never upload source or spend remote-build credits
implicitly. Dirty builds need a content digest, not just a Git hash or version,
to distinguish their identity. Report client and running-daemon identities and
negotiate required protocol capabilities rather than requiring matching versions.

Development selection is host-wide and affects all sessions. Defer activation
until all work is safe and display that scope. Parallel stable and dev daemons
with isolated sockets and state can be a later feature. Do not run two daemons
against the same live session and credential stores as a shortcut.

## Implementation order

1. Add the approved release manifest and verified host updater to managed-cloud
   provisioning. Preserve `JCODE_NO_UPDATE=1`. Use a bundled last-good runtime if
   the manifest is temporarily unavailable. Update and verify the bootstrap
   release separately rather than guessing a new digest.
2. Add running-version health reporting, persistent update state, supervisor
   rollback, and shared install locking. Integrate client preflight with that
   lock and managed policy so it cannot race or bypass pins and quarantines.
3. Deliver the daemon idle-activation gate and explicit managed target support.
   Gate automatic in-process activation on negotiated capability, so older
   daemons defer to a normal restart instead of being interrupted.
4. Roll out the updater to existing hosts through an explicit, idempotent fleet
   migration. Changing bootstrap alone affects only newly provisioned hosts.
   Do not reboot hosts or modify customer workspaces for this migration.
5. Enable internal-cohort activation, exercise rollback, then widen rollout.

## Acceptance checks before rollout

- A newly approved arbitrary release is discovered without a client upgrade.
- A stopped VM is not woken by rollout and checks policy when normally resumed.
- Missing, malformed, incomplete, tampered, or unavailable releases preserve the
  last-good runtime and leave bootstrap able to start it.
- Concurrent timer and client preflight installs serialize and retain launchers.
- Streaming turns, tool calls, detached workers, transfers, and non-survivable
  background jobs defer activation. Starting new work cannot race the idle gate.
- Idle activation reconnects clients and preserves sessions, credentials,
  queued input, provider selection, and workspace contents.
- Failed startup restores the previous release, reports the actual running
  version, and quarantines the bad release across restarts.
- Cohort assignment is stable, pins are honored, pause stops promotion, and
  existing hosts receive the updater without resetting their machines.

## Scope

This policy applies to managed Jcode Cloud hosts, not arbitrary user SSH hosts,
local development sessions, hosted compiler sandboxes, or desktop installations.
No cloud configuration or running host was changed by this design document.
