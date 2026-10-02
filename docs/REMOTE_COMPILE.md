# Remote compilation and cloud-compute credits

`compile_remote` uploads a source snapshot to Jcode's authenticated API and runs
one build in an isolated Linux sandbox. It returns compiler stdout, stderr, exit
code, and server-metered usage. It never executes the supplied command locally.

## Subscription-aware tool surface

The tool remains discoverable when signed out. Its description asks the agent to
explain the Jcode subscription requirement and link to <https://jcode.sh/pricing>.
Existing subscribers can sign in with `jcode account login`. The tool does not
open checkout, subscribe, or purchase credits automatically.

Before exposing the schema, the harness checks `/v1/me` with the configured Jcode
account credential. Guidance distinguishes signed out, subscription required,
available, service not enabled, and account status unknown. Account checks are
cached for 60 seconds by a fingerprint of the key and API base, not by model
provider. Using a non-Jcode model does not prevent use of a Jcode subscription.
Locked tool snapshots refresh this description when account state changes,
including sessions using deferred MCP tools.

Every compilation checks access again **before reading or uploading source**.
Neither a cached tier nor a local environment flag authorizes a build. The
server independently authenticates the request, verifies paid entitlement,
reserves cloud credits, and admits sandbox capacity. An unavailable API fails
closed, without representing the user as unsubscribed.

## Tool calls

Check access, the shared credit balance, and recent itemized compute usage
without uploading source or starting a machine:

```json
{"action":"status","intent":"Check remote build access"}
```

Compile a Git checkout:

```json
{
  "action":"compile",
  "command":"cargo check --locked",
  "path":".",
  "timeout_seconds":300,
  "intent":"Check the project on remote compute"
}
```

`path` must identify the Git repository root. The selected server template must
contain the required compiler and dependencies. The deadline is 1 to 600 seconds
(default 300), with additional bounded setup time in the server reservation.
A nonzero compiler exit is a completed build result, not transport success
masquerading as build success.

Source snapshots currently require a Unix client (Linux or macOS) for
descriptor-relative, no-symlink file reads. Other platforms fail closed.

## One compute balance

Remote compilation and future cloud-agent execution share **one cloud-compute
credit ledger**. This balance is separate from model inference billing. Every
entry is tagged with its workload and request ID so usage can be consolidated
and itemized without double charging retries.

The backend reserves the maximum allowed machine lifetime at an operator-set
rate before starting compute, then settles against server-measured lifetime and
releases the unused reservation. Failed compilations still consume credits for
the compute they used. An insufficient balance returns HTTP 402. There are no
automatic top-ups, overdrafts, or implied dollar conversion for these units.

The initial ledger supports explicit audited, idempotent operator grants.
Included plan allocations and any credit purchase UX need a product decision
and are not silently fabricated. Credit units and rates must be configured
before rollout. Uncertain/lost-job reservations are handled conservatively by
the backend rather than refunded while a machine might still be running.

## Source sharing and privacy

Only use remote compilation after the user requests remote builds or authorizes
source sharing. The snapshot includes current tracked files and nonignored
untracked regular files, so uncommitted changes are included. Deleted tracked
files are omitted. Git metadata, common build/cache directories, symlinks, and
common credential files are excluded even if tracked. Local environment
variables, account keys, SSH credentials, and `.git` history are not forwarded.

Exclusions **cannot prove arbitrary source files contain no secrets**. Review
sensitive projects before sharing. The backend executes code with an isolated
third-party sandbox provider (initial adapter: E2B), never in the account API
process or a personal SSH build machine. The operator must provision a clean
compiler template with no baked-in secrets.

Uploads are bounded to 20 MiB of raw file contents, 2 MiB per file, and 10,000
files, with a 30 MiB encoded request limit. Paths containing control characters
are rejected before upload. Response bodies are bounded to 1 MiB. No automatic HTTP redirects or
submission retries are followed. A timeout/disconnect may leave a bounded job
running and consuming reserved credits. Inspect usage before submitting a new
job, which has a different request ID and can incur another charge.

## Initial release boundary

This version uses a fresh isolated sandbox per job. It does **not** yet provide
persistent dependency/build caches, artifact download, cross-compilation setup,
or an actual cloud-agent runner. The ledger supports the `cloud_agent` workload
for a future runner. Builds requiring Git history, symlinked source, ignored
files, private dependency credentials, or network downloads may need a suitable
prebuilt template or local compilation.

The backend capability is off by default. Operators must apply the database
migration and configure a provider credential, compiler template, rate, and
capacity controls before enabling it. Accounts also need explicit credit grants
before a job can be admitted, even when the capability is advertised. Adding
the tool does not deploy or provision cloud infrastructure.
See `workers/api/REMOTE_COMPILE.md` and the compute-credit documentation in the
`solosystems-backend` repository for the server deployment contract.
