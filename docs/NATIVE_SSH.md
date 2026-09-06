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

The remote host must have its own provider configuration. Run `jcode login` on
that host when needed. Native attach does not copy or forward local provider
credentials, AWS credentials, an SSH agent, or repository contents.

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
