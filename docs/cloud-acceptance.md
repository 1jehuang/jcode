# Cloud handoff acceptance

`python3 scripts/test_cloud_acceptance.py --binary PATH --repeat 2` is the
Linux acceptance entry point. It runs the CLI, agent, credentials and TUI layers
against the supplied executable, twice with fresh fixture directories. It is a
regression gate, not evidence that a managed cloud service or live login works.
A workflow definition or successful build alone is not an acceptance result.

## Coordinated group commands

`/cloud` and `jcode cloud move` include the selected session's associated swarm.
Use `/cloud --dry-run` to preview, and repeat `--include-session <id>` to opt in
independent sessions sharing the same checkout. `--session-only` explicitly
selects the legacy single-session path and conflicts with `--include-session`.
The coordinator owns environment setup and the temporary local resource bridge;
members share the resulting repository-keyed environment instead of each
requesting credentials or repeating setup.

Group migration requires compatible daemons and a safe idle boundary. It must
fail closed when it cannot reserve that boundary. A failed or interrupted
activation can leave source sessions fenced deliberately: retry the same move
to recover, rather than deleting leases or starting both copies. The initial
group implementation rejects individual member returns, and full group return
is not implemented. Ungrouped single-session move/return remains available.
Imported workers stay paused until the coordinator finishes shared setup and
resumes their existing IDs. Only the initiating terminal attaches automatically;
other terminals must reconnect to their migrated sessions.

## What runs

| Layer | Mechanism and assertions |
| --- | --- |
| `cli` | `scripts/test_cloud_move_e2e.sh`: real cloud move/return commands and daemons, with a local transport script into a **bubblewrap** filesystem sandbox instead of SSH. Exercises dry run, repository/index/untracked state, session and todo transfer, ownership fencing, round-trip merge and conflict handling. |
| `agent` | `scripts/test_cloud_move_agent_e2e.sh`: real agent turns before move, in the bubblewrap destination and after return. Checks conversation continuity, cloud notice in provider context and refusal to run the away local copy. Transport is local, not SSH. |
| `credentials` | `tests/test_cloud_credentials.py`: synthetic OpenAI/Claude OAuth and router credentials over local pipes. Checks round trips, all-account transfer, permissions, secret-free archives/diagnostics, stale and expired tokens, dry run, identity/refresh conflicts, ownership rollback, one-way router transfer and failure before transfer with an incompatible remote. No production authentication or refresh endpoint is used. |
| `group` | `scripts/test_cloud_group_e2e.sh`: real local/cloud daemons with isolated homes and a bubblewrap transport. Checks automatic swarm selection, explicit independent-session inclusion, excluded sessions, one shared repository and environment replay, preserved conversation/todo/swarm state, preview, and safe retry after a lost activation response. No live cloud account or model is used. |
| `tui` | `scripts/cloud_acceptance_tui.py`: the **actual interactive TUI in tmux**, a real loopback OpenSSH server and client, isolated local/cloud homes, real daemons, and a user/mount namespace providing separate repository contents at the same absolute path. No cloud-transport override is used in this layer. |

The agent and TUI use `scripts/fake_openai_server.py`, a deterministic local
OpenAI-compatible provider. It records requests and can hold/release turns.
Provider context assertions test the handoff protocol, not model intelligence.
The TUI submits real terminal keystrokes. It reads the client's debug `state`
endpoint only to observe idle boundaries, never to seed or mutate client state.
An old answer still present in terminal history cannot satisfy a new destination
turn: the harness requires a fresh agent HTTP request with the cloud continuation.
Connectivity probes and background memory extraction do not count as agent turns.

The TUI scenarios are:

- `idle_round_trip`: two cloud/local cycles, cloud and concurrent local files,
  migration epochs and original conversation context.
- `midturn_auto_poke`: prepare while a provider turn is held, retain local
  ownership until it completes, automatically continue on cloud and transfer todos.
- `cancel_prepared`: cancel a prepared move and continue locally without migration.
- `draft_preserved`: an unsent composer draft prevents automatic migration and can
  still be submitted locally.
- `prepare_outage_retry`: an injected SSH failure during preparation leaves local
  ownership intact, then a retry moves successfully.
- `transfer_outage_retry`: an SSH failure after preparation preserves local
  ownership and epoch, then a retry moves successfully.
- `destination_binary_retry`: missing and protocol-incompatible destination
  binaries fail before ownership changes, then a corrected destination works.

With the current seven TUI scenarios, a full `--repeat 2` invocation records 22
stage results. Credential test cases are grouped into one stage per repetition.
The runner continues after a failed stage and returns nonzero if any stage fails.
Missing tools or failed namespace/bubblewrap probes are **failures, never skips**.

## Linux prerequisites and isolation

Install Python 3, Bash, Git, tmux, OpenSSH client/server, bubblewrap and util-linux
(`unshare`, `mount`, `timeout`). The TUI fixture invokes `/usr/bin/sshd`; where the
package installs `/usr/sbin/sshd`, provide the corresponding symlink. OpenSSH also
needs `/run/sshd`. Package installation and this host setup may need administrator
privileges. **Do not run the acceptance runner with sudo or as root.**

Unprivileged user and mount namespaces must work. Ubuntu's AppArmor user-namespace
restriction can block them. The workflow conditionally disables
`kernel.apparmor_restrict_unprivileged_userns` and enables
`kernel.unprivileged_userns_clone` only on its disposable GitHub-hosted runner.
Do not copy that system-wide policy change onto a shared workstation or production
host. Use a disposable Linux VM with suitable namespace policy instead.

Fixtures allowlist process environments, generate their own SSH keys, use only
synthetic credentials and keep separate runtime/home directories. Ephemeral Unix
sockets use short private directories under `/tmp` to avoid Linux socket path
limits even when artifact directories are deeply nested. Those socket directories
are removed after their fixture processes stop. Fixtures are not
intended to access your real accounts, shared daemon or managed VM. Never add
production tokens, account files or SSH keys to the fixtures or workflow.

## Manual use and focused diagnosis

Check the test machinery itself with:

```bash
python3 tests/test_cloud_acceptance_harness.py
```

These fast contracts check failure propagation, timeouts, report generation,
provider gates, short socket paths, and fixture-only process cleanup. They are not a substitute for
running the actual acceptance matrix below.

Use an already-built executable, with an absolute path recommended:

```bash
python3 scripts/test_cloud_acceptance.py \
  --binary /absolute/path/to/jcode --repeat 2 \
  --artifacts "$PWD/cloud-acceptance-results"
```

`--artifacts` must name a **new** directory. Without it, the runner creates one
under `JCODE_SCRATCH_DIR`, or the system temporary directory, and prints its path.
Run a single TUI case or a single lower layer when diagnosing a failure:

```bash
python3 scripts/test_cloud_acceptance.py --binary /absolute/path/to/jcode \
  --layers tui --scenario transfer_outage_retry --repeat 1
python3 scripts/test_cloud_acceptance.py --binary /absolute/path/to/jcode \
  --layers credentials --repeat 1
python3 scripts/test_cloud_acceptance.py --binary /absolute/path/to/jcode \
  --layers group --repeat 1
```

Other `--scenario` values are listed above. `--scenario` filters only TUI scenarios,
so pair it with `--layers tui` for a one-case run. The underlying credential suite
also supports unittest's individual test selection:

```bash
python3 tests/test_cloud_credentials.py /absolute/path/to/jcode \
  CloudCredentials.test_return_conflict_restores_cloud_ownership
```

`--timeout` defaults to 240 seconds per CLI, agent or credential stage. TUI waits
have their own bounded deadlines and are not controlled by that flag. If a run is
externally killed, inspect the last saved evidence rather than assuming a complete
report or successful cleanup.

## CI and evidence

`.github/workflows/cloud-acceptance.yml` runs daily at 09:23 UTC, on manual dispatch,
and on relevant PRs/pushes targeting `main` or `master`. Its intentionally broad
`src/**`, `crates/**`, `scripts/**` and `tests/**` filters include TUI input/remote
handling, cloud CLI, transport, credential code and harness changes. Cargo/build
configuration and this documentation also trigger it.

The workflow follows existing CI conventions: recursive checkout, stable Rust
with `rust-src`, Rust cache, wrapper disabled and clang/mold linking. It installs
the repository-pinned justrust client and uses its local CI build path without a
hosted account, building the locked release `jcode` binary for Linux x86_64. It
checks module declarations before building. Existing quality CI remains responsible
for the code-size/test-size and other ratchet budgets, this gate does not replace
them. Build, acceptance and whole-job limits are 75, 40 and 120 minutes respectively.

The artifact upload runs with `always()`, retaining available evidence for seven
days, including hidden files. A failure before the runner starts may leave no
artifact. The bundle includes `report.json`, `junit.xml`, per-stage output logs,
provider requests, SSH logs, captured TUI `frame*.txt`, and retained fixture homes
and workspaces. The root evidence directory is mode 0700 locally. Credential-suite
temporary homes are normally cleaned by unittest, while other fixture homes remain.
Those homes can contain **synthetic-only credentials and disposable private SSH
keys**. Artifact retention is deliberate for diagnosis, not an invitation to use
real secrets. An externally timed-out run may have only partial reports/frames.

## Explicit non-coverage

This gate does not provision, suspend, resume or delete actual managed cloud VMs.
It does not validate provider availability, production OAuth login/refresh, real
account authorization, public-network SSH behavior or nondeterministic live model
responses. Passing it would establish only the isolated layers and assertions
above. Record actual run results separately, with the tested binary/version and
retained evidence, before claiming acceptance success.
