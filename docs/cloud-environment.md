# Cloud environment requirements

Moving a session or replacing a checkout does **not** make two machines identical.
Check the runtime, tools, files and credentials before continuing work. Install
missing dependencies deliberately and document runtime/package setup in project
setup documentation. Keep only typed resource requirements in the JSON manifest. Do not put
secret values in messages, shell arguments, tool results, or this document.

## Request protocol

The cloud environment tool, when available, accepts metadata-only JSON:

```json
{"action":"status"}
```

```json
{"action":"sync"}
```

```json
{"action":"fetch","resource":{"kind":"project_file","path":".env.local"},"reason":"Required by development server"}
```

```json
{"action":"fetch","resource":{"kind":"env_var","name":"SERVICE_TOKEN"}}
```

```json
{"action":"fetch","resource":{"kind":"credential","name":"github"}}
```

Unknown fields, absolute paths, traversal, symlinks and reserved control paths
are rejected. Requests are limited to 8 KiB. `reason` is optional, bounded
metadata. It is not persisted or reflected in status output. Do not include
secrets in it. Environment injection controls such as `PATH`, `HOME`, `LD_*`,
`BASH_ENV`, `SSH_*`, and `JCODE_*` are denied.

The credential allowlist is fixed:

| Name | Files, relative to the user's home |
| --- | --- |
| `github` | `.config/gh/hosts.yml` |
| `aws` | `.aws/credentials`, `.aws/config` |
| `npm` | `.npmrc` |

There is no arbitrary home-file or keychain access and no SSH private-key
transfer. Project files must be inside the repository, not Git or Jcode control
metadata. Missing AWS components may be omitted, but at least one must exist.

## Temporary callback after handoff

A supporting local TUI may grant access for 15 minutes after a session handoff.
This is conditional, not guaranteed availability. The TUI invokes:

```text
jcode --no-update --no-selfdev cloud env fulfill --session SESSION_ID
```

The request is read from stdin, not a command argument. The caller must set both
`JCODE_CLOUD_ENV_SESSION=SESSION_ID` and `JCODE_SSH_REMOTE` to the lease's exact
destination. The command rejects missing, returned, stale, zero-epoch, future,
or expired leases and invalid repository roots. The receiver independently
requires the same session epoch and repository to be owned on the remote host.
Expiry or disconnection revokes local access, not resources already copied:
cloud files and credentials remain there until explicitly removed or rotated.

Every callback response is bounded, nonsecret JSON, for example:

```json
{"ok":true,"code":"received","applied":1,"skipped":0}
```

Errors return `{"ok":false,"code":"unavailable","applied":0,"skipped":0}`
without raw diagnostics. A zero process exit means a JSON response was produced,
not that transfer succeeded: check `ok`. `status` confirms the local callback is
eligible and counts saved requirements, not remote service health.

## Explicit local push

When no callback is available, including repository-only replacement, run this
on the **local source machine**, from the repository. A replacement agent remains
local and should run this command itself with its local tools when available,
not ask the user to perform work it can do:

```sh
jcode cloud env push --host my-cloud --request '{"action":"fetch","resource":{"kind":"credential","name":"github"}}'
jcode cloud env push --host my-cloud --request '{"action":"sync"}'
```

The checkout uses the same absolute repository path on both machines. SSH uses
the existing cloud target configuration. Values travel only over private SSH
stdin into hidden `cloud env receive`, never in argv, session archives, temporary
payload files, logs, or stdout. Remote stderr is discarded. Transport I/O has a
30-second deadline. Fulfillment honors the TUI's `JCODE_SSH_BINARY` and remains
inside its disconnect-cancellation process group. The private payload
is versioned, bounded to 4 MiB including encoding, and validates every source,
resource, decoded value and destination before beginning writes.

Publication is atomic per file with mode `0600`. Existing files, including
different credentials, are skipped and never overwritten. Existing stored
environment variables are also preserved. A multi-file transfer is not a
transaction: an I/O failure can leave earlier successfully published files.
Retrying is safe because publication does not overwrite existing files.

## Saved requirements and environment

### Shared workspace after a group handoff

A migrated group uses one canonical repository root, not a checkout per member.
Project files, fixed home credentials, saved requirements and the private
repository-keyed environment are shared. The transfer path can inject
`cloud_environment::group_migration_guidance(session_id, coordinator_session_id)`:
the matching coordinator owns workspace setup and resource requests, while
members use the hydrated environment and report specific missing identifiers
to that coordinator. Single-session moves retain `migration_guidance`.

Only the coordinator owns any temporary local bridge. Members must not each
repeat setup, sync, or request a separate local push. The coordinator checks
actual state, syncs saved requirements once when needed, deduplicates member
requests, and confirms success before members proceed. This is coordination
guidance, not an exactly-once setup guarantee or proof of parity. A failed or
partial setup may need a safe retry. New environment values are visible to
subsequent tool processes, not processes already running.

Group membership never authorizes a callback. The existing exact session,
destination, epoch, repository and expiring lease checks still apply. Do not
copy a coordinator's callback grant to members. Without a callback, the
coordinator arranges an explicit push from the local source machine.

Concurrent receives acquire a repository-keyed hydration lock before validating
the payload and publishing resources. Acquisition waits at most five seconds,
then returns the same nonsecret unavailable status. The separate map locks avoid
recursive locking. Lock release follows guard drop or process exit, without
stale lock-file deletion. Repeated pushes remain safe no-clobber operations:
serialization does not suppress network transfers or cache authorization, and
each receive revalidates its own lease. It also does not make multi-file writes
transactional. No secrets or free-text reasons are used as lock names.

`.jcode/cloud-environment.json` contains only typed resource identifiers:

```json
{
  "requirements": [
    {"kind":"project_file","path":".env.local"},
    {"kind":"env_var","name":"SERVICE_TOKEN"},
    {"kind":"credential","name":"github"}
  ]
}
```

Fetch records the requirement locally and, on successful receiving, remotely.
Subsequent sync and integrated move/replacement synchronization replay these
requirements. At most 128 requirements are accepted. Values and free-text
reasons are never stored in this manifest. Names and paths are metadata and may
still be sensitive in your organization, so review them before committing.

Environment values are stored privately outside the repository under the Jcode
state directory, keyed by canonical repository root, so loads from nested working
directories see the same values. Stored values are checked again for NUL bytes
and size limits. Manifest and environment read-modify-write operations use
cross-process file locks and atomic replacement. The base API
`cloud_environment::load_environment(root)` returns that repository's named
values for shell integration. It does not expose them in status output. Do not
print or serialize its result into transcripts. Missing local resources cause
sync to report unavailable rather than silently pretending the environments
match. Reconcile differing existing files manually if necessary.
