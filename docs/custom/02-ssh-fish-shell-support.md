# [02] SSH remote command supports fish login shell

## Requirement

`jcode --ssh <host>` (and the SDK / TUI auth-remote transports) must work
when the remote account's login shell is fish. Previously the remote
command was hard-coded POSIX syntax
(`PATH="..."; export PATH; exec ...`), which fish rejects with
`Unsupported use of '='` and the connection dies before the handshake.

## Behavior

- Before spawning the remote command, the transport runs one probe:
  `ssh <opts> <host> env`. `env` is a single bare command valid under
  every login shell, and its output carries `SHELL=<path>`.
- If the probed shell basename is `fish`, the remote command uses
  fish-native syntax:
  `set PATH "$HOME/.local/bin" "$HOME/.cargo/bin" $PATH; exec ...`
- Any other shell, or any probe failure/timeout, falls back to the
  existing POSIX command — identical to previous behavior.
- The probe result is captured once per connection setup and reused for
  reconnects (native SSH listener) — no extra round trip per reconnect.

## Config surface

None. Detection is automatic; there is no override flag.

## Affected files

- `src/cli/ssh_transport.rs` — `RemoteShell`, `probe_shell()`,
  per-shell remote command; probe runs in `connect_with_workspace`.
- `crates/jcode-sdk/src/ssh.rs` — same pattern for `jcode api --stdio`
  (`SshConnectOptions::command(shell)`, blocking `probe_shell()`).
- `crates/jcode-tui/src/tui/app/auth_remote/command.rs` — same pattern
  for remote auth operations (`Target::command(shell, ...)`).
- `crates/jcode-sdk/src/ssh_integration_tests.rs` and unit tests —
  updated for the new `command(shell)` signature.

## Upstream status

General fix, upstream candidate. The fish/POSIX split and the `env`
probe are not fork-specific.
