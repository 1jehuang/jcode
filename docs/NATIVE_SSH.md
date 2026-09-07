# Native SSH client

The native SSH client runs the terminal UI on your local machine and keeps the
workspace, tools, model credentials, and agent execution on the SSH host. This is
remote **attach**, not workspace or live-process migration.

## Usage

Install a compatible Jcode binary on both hosts and configure ordinary OpenSSH
key authentication and a verified host key first. Native attach is noninteractive
at the SSH layer and will refuse unknown host keys or missing authentication.

```sh
jcode --ssh dev --remote-working-dir /srv/jcode
jcode --ssh dev --remote-working-dir /srv/jcode self-dev
jcode --ssh dev --resume session_remote_id
```

`dev` is an SSH config alias or `user@hostname`. `--ssh-binary /path/to/jcode`
selects the remote executable. With no workspace argument, the bridge's remote
working directory is used, never the local client's directory. An explicit resume
ID is resolved on the remote server, not in local session storage.

Authenticate the remote host directly from the local TUI with `/login`. It uses
the same inline Login picker as local mode: arrow keys move, typing filters,
Enter selects, and Esc cancels. The remote picker offers six supported OAuth
providers plus **Import local OpenAI login** and **Import local Claude login**.
Its destination notice names the SSH host. Provider status is fetched from that
host with `jcode auth status --json`, never from the laptop's credential stores.
Unknown or unavailable status is shown as unknown, not as signed out. Only
provider state and fixed method labels are displayed, not remote account labels,
credential paths, or raw remote errors.

You can also choose a provider explicitly, for example `/login openai` or
`/login claude`. The browser
approval happens on your laptop, while pending login records, token exchange, and
saved credentials stay on the SSH host. Treat authorization URLs as sensitive and
do not share them. Paste the returned callback URL or authorization
code into the pending login prompt, not into an ordinary chat message. OpenAI
requires the full callback URL, even if the browser reports that its localhost
callback page cannot be reached. `/cancel` cancels the pending login.

This remote login surface supports the scriptable OAuth providers OpenAI, Claude,
Gemini, Antigravity, Google, and Copilot. Google additionally requires its OAuth
client configuration to be set up on the VM first. Other credential routes still
require `jcode login` on the host. Native attach never automatically copies local
provider credentials. AWS credentials, SSH agents, and repository contents are
never forwarded by this feature.

Each native login attempt has its own remote flow ID, so two clients cannot
replace each other's pending OAuth state. Callback input travels over SSH stdin,
not command-line arguments or chat messages. Cancelling a pending attempt removes
only that attempt's state, not another login or previously saved credentials.
Cancellation is not logout and cannot revoke credentials already issued by a
completed exchange. Closing the UI terminates its owned authentication subprocess.

### One-time import of a local login

To avoid another browser login on a trusted SSH host, open `/login` **inside the
native SSH TUI** and choose an **Import local … login** row. `/login --import-local`
opens a smaller picker containing just those two import choices. Both routes lead
to the same explicit confirmation, and neither reads local credentials while
the picker is open. Direct commands remain available:

```text
/login --import-local openai
/login --import-local claude
```

Read the destination-host warning, then type exactly `confirm` and press Enter.
Before confirmation, no credential export is performed. Esc or `/cancel` at the
confirmation prompt reads and copies nothing. After confirmation, the selected
active account is read from the laptop's Jcode-managed OAuth store and sent over
the pinned-host SSH connection through stdin, never command-line arguments or
chat messages. The remote CLI stores it privately and the TUI requests a remote
provider/catalog refresh. Reopen older client windows after updating both binaries.

Important boundaries:

- This is an explicit **one-time copy**, not automatic startup synchronization.
  The remote host receives usable credentials, including refresh credentials.
  Only do this for a host you trust with that provider account.
- OpenAI and Claude **Jcode-managed OAuth accounts only** are supported. Other
  accounts, external-tool stores, keychains, environment/API keys, AWS credentials,
  and general configuration are not imported. Missing, malformed, or expired
  source credentials are refused. Use ordinary remote `/login` instead.
- Any existing selected-provider destination store is refused, even an empty or
  malformed file. Claude's shared `auth.json` is also refused when it only contains
  other providers. This conservative rule prevents overwriting concurrent changes.
  No `--overwrite` switch exists. Other credential stores are preserved.
- New credential files use mode 0600 within a 0700 Jcode data directory, with
  atomic no-replace publication. Secret transport data is bounded to 64 KiB and
  is not logged or written as a transport file. Import is currently Unix-only.
- Acknowledged import means **credentials were stored**, not that the provider
  accepted them. OAuth refresh-token rotation can cause either machine's copied
  login to stop working. Independent remote `/login` avoids sharing refresh state.
- Once transfer starts, cancellation/disconnection cannot promise rollback.
  Check the remote login state before retrying. There is no automatic retry or sync.

The receiver command `jcode auth import --provider openai --stdin --json` is for
the native client, not for pasting tokens into a shell. There is deliberately no
CLI export command that prints credentials.

## Protocol and compatibility

The client creates a private local Unix socket adapter. Each native connection
uses an owned `ssh -T` child to invoke `jcode server stdio` remotely. That command
connects to or starts the native daemon, checks its native SSH capability, emits a
bounded versioned handshake, and transports native JSON frames over stdio.

This is deliberately separate from `jcode api --stdio`, whose harness API is used
by the SDK and is not wire-compatible with the TUI protocol. The bridge checks the
actual daemon's capability, not merely the bridge executable's version. An old
shared daemon is refused rather than silently reloaded or killed.

To test or deploy alongside an existing daemon without interrupting it, start a
matching daemon in a separate `JCODE_RUNTIME_DIR` and point
`--ssh-server-socket /remote/runtime/jcode.sock` at it. A socket override by itself
does not isolate the daemon lock. A remote executable wrapper may instead export
both `JCODE_RUNTIME_DIR` and `JCODE_SOCKET` before executing the new binary.

## Disconnect behavior

SSH clients explicitly opt into `Subscribe.continue_on_disconnect`. If a turn is
active when that client disappears, the server retains the turn's supervisor.
A new client can attach to the same session, request full remote history, see new
events, and cancel the active turn. Ordinary local clients retain their existing
disconnect semantics.

Limits:

- There is no event-cursor replay. Reconnect refreshes remote history rather than
  trusting local transcripts or replaying every missed delta.
- Connection-owned stdin prompts do not migrate across disconnect. Pending
  responses fail closed instead of being silently approved.
- Idle detached sessions are not kept alive indefinitely. Completed session
  history remains available through normal persistence.
- Stopping the VM or restarting the daemon does not preserve in-flight processes.
  Persistent disk preserves saved files and history, not RAM or running commands.
- An external VM SSH-idle shutdown policy still applies. If it stops the VM after
  an hour without SSH connections, a detached turn cannot override that policy.

Closing the client cleans up its SSH bridges and private socket, not the remote
shared daemon. SSH keepalives detect a dead connection. Native attach is currently
supported on Unix clients.

## Host boundary

The TUI identifies the remote host and does not load, save, or mark remote sessions
in the laptop's session store. Remote sessions do not launch laptop-local provider
onboarding. Local-only account, configuration, file-opener, new-terminal, and
reload actions are guarded rather than interpreting remote paths locally.
Provider/tool startup overrides and `self-dev --build` are rejected in SSH mode.
Use supported remote commands or an explicit shell on the host instead.

## Verification

Targeted suites:

```sh
cargo test --lib cli::ssh
cargo test -p jcode-protocol
cargo test -p jcode-tui --lib ssh_remote -- --test-threads=1
cargo test -p jcode-app-core --lib client_disconnect_cleanup -- --test-threads=1
cargo test -p jcode-app-core --lib client_lifecycle -- --test-threads=1
cargo test --test e2e disconnect:: -- --test-threads=1
```

These cover protocol, routing, transport, and controlled-provider lifecycle
behavior. They do not by themselves establish that an actual SSH-launched TUI,
remote provider login, and real remote tools work end to end. Deployment reports
must separately identify real SSH/TUI observations, controlled-provider evidence,
and any blocked provider-backed acceptance.

### Real CLI acceptance

`tests/test_native_ssh_cli.py` is opt-in and uses actual OpenSSH, a built CLI, and
a real PTY. Set `JCODE_NATIVE_SSH_BINARY`, `JCODE_NATIVE_SSH_HOST`,
`JCODE_NATIVE_SSH_REMOTE_BINARY`, and `JCODE_NATIVE_SSH_CWD`. The remote wrapper
should select an isolated daemon runtime. With no configuration it skips without
network access. The script only sends context-only messages, never inference.

On 2026-09-06 this passed between an Arch Linux client and an Ubuntu EC2 host:
capability handshake, piped EOF/final Pong, remote context persistence and fresh
reattach, invalid-cwd/unsupported-flag refusal, actual TUI remote-history display,
no local transcript, 0700 adapter directory, SSH child/socket cleanup on both
`/quit` and SIGHUP, and daemon/session survival afterward. The installed niri
shortcut was separately exercised with the actual kernel key chord.

175 focused Rust tests passed, including nine real-socket disconnect cases with
a controlled provider. Those establish lifecycle behavior, not live external
model inference. Provider-backed development still requires remote login and was
not claimed as passed by the context-only SSH acceptance.

### Remote login acceptance

`tests/test_native_ssh_login.py` requires the separate explicit opt-in
`JCODE_NATIVE_SSH_LOGIN=1`, plus the local binary, SSH host, workspace, and
`JCODE_NATIVE_SSH_LOGIN_REMOTE_EXECUTABLE` (the actual remote ELF, not a wrapper).
It creates a private remote home/runtime and never uses the user's credentials.
The current harness checks the shared inline picker and its six OAuth/two import
choices before the isolated login scenarios. Updating this harness does not by
itself establish that the new picker has passed real SSH acceptance.

On 2026-09-06, the real local PTY and EC2 SSH workflow passed:

| Requirement | Observed check |
| --- | --- |
| `/login` authenticates the remote host | Provider choice and `/login openai` generated an OAuth URL matching the VM's private PKCE/state record. |
| Callback input stays private | Synthetic callback arrived through SSH stdin, failed real CLI state validation before exchange, and never appeared in local/remote logs, transcripts, or prompt history. |
| Cancellation is scoped | Bare picker and pending-flow cancellation passed, with an unrelated legacy pending file unchanged. |
| Failure cannot trigger local auth | Remote error output was redacted, unsupported providers were refused, and no local credentials were created. |
| Login does not leak subprocesses | `/quit` reaped owned SSH processes and removed the private adapter socket. |

78 focused Rust tests passed across CLI login (24), CLI arguments (34), and SSH
TUI routing/authentication (20). Browserless login QR/JSON command-level tests
also passed against the installed binary. Unit tests cover successful remote
provider refresh, but **real OAuth approval, successful external token exchange,
and provider-backed inference were not completed**. Those require the user's
provider approval and are not implied by the safe invalid-state acceptance.


A follow-up real SSH run at 08:42 UTC also passed `/login claude` initiation and
scoped cancellation in a fresh local PTY. The VM verified the URL's PKCE challenge
against its private pending file, and the harness matched only a URL hash in
memory. The full URL was not printed or persisted in local/remote artifacts.
Claude's existing OAuth contract includes the verifier in the authorization URL's
state, so the OpenAI-specific "verifier never transferred" assertion does **not**
apply to Claude. No Claude callback or token exchange was attempted. The harness
now isolates scenarios in fresh PTYs and waits for unambiguous cancellation
messages to avoid mistaking historical terminal redraws for completion. All 18
offline harness checks and the expanded live acceptance passed.

### Local credential import acceptance

`tests/test_native_ssh_import.py` requires `JCODE_NATIVE_SSH_IMPORT=1`,
`JCODE_NATIVE_SSH_BINARY`, `JCODE_NATIVE_SSH_HOST`, `JCODE_NATIVE_SSH_CWD`, and
`JCODE_NATIVE_SSH_IMPORT_REMOTE_EXECUTABLE` (the actual remote ELF). It creates
fresh local and remote homes with unmistakably synthetic credentials. Its safety
wrapper refuses anything except the selected synthetic payload before invoking
the real receiver CLI. No personal credentials are imported by the test.
The current harness adds bare-picker arrow selection and import-only filtered
selection, each followed by cancellation at the destination warning, for both
providers. It checks that no import subprocess started, no account labels were
displayed, and the synthetic local/remote stores remained unchanged. It retains
the direct-command cancel, confirmed import, and repeat/refusal scenarios. Run
`python3 tests/test_native_ssh_import.py --self-test` for offline harness safety
checks. Real PTY/SSH acceptance still requires the explicit opt-in above.

On 2026-09-06 at 11:07 UTC, the real local TUI, OpenSSH, remote CLI and daemon
passed all six scenarios: cancel, confirm/import, and repeat/refuse for each of
OpenAI and Claude.

| Requirement | Observed check |
| --- | --- |
| Explicit opt-in | A fresh real PTY displayed the destination warning. Cancellation invoked no import subprocess and created no selected-provider store. |
| Selected-provider transfer | Exact synthetic selected-provider credentials crossed stdin only after `confirm`; the other local and remote credential stores remained byte-identical. |
| Persistent private storage | The receiver wrote a 0600 store. A separate real `auth status --json` reported the selected provider as available. This indicates configured credentials, not provider acceptance. |
| No silent overwrite | A second confirmed import failed and preserved the first store unchanged. |
| Secret handling | PTY output, observed process arguments, logs/history and other local/remote artifacts contained no synthetic tokens outside the exact credential stores. |
| Process cleanup | Each of six fresh PTYs exited via `/quit` and reaped owned SSH children and adapter sockets. The two isolated test daemons were separately stopped using their verified PID/start-time/home identities. |

Supporting checks passed: 11 base transfer tests, 35 CLI argument tests, 11
startup tests, 28 SSH TUI tests, and 9 real built-CLI receiver tests. The receiver
tests include concurrent no-replace publication, malformed/oversized input,
terminal refusal, a real 30-second stalled-pipe deadline, and invalid/help
arguments leaving existing file contents, modes, timestamps and directory trees
untouched. Eleven offline SSH harness safety tests also passed.

These checks establish the import workflow with synthetic credentials. They do
not establish successful provider-backed inference, personal token validity, or
long-term refresh-token coexistence. Closed loopback HTTP proxies were used as
defense in depth, not as proof of zero network packets.
