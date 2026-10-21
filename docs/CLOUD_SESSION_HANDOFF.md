# Cloud session handoff and OAuth accounts

`/cloud` (or `jcode cloud move`) copies the current session's **selected Jcode-owned OpenAI or Claude OAuth account** to the chosen SSH host. Choosing the destination and running the move authorizes that account copy. Use only a host you trust with the account's access and refresh tokens.

- The original account label is retained and the migrated session is pinned to it. A missing explicitly selected account is an error, not a reason to choose another login.
- Tokens travel in a separate SSH stdin/stdout exchange. They are not included in the session/repository archive, manifests, command arguments, progress messages, or diagnostic output. Remote credential-command stderr is deliberately discarded to prevent accidental token reflection.
- The destination store uses private permissions and atomic publication. Other accounts and the active selection are preserved. A conflicting identity is not overwritten. Current Jcode processes coordinate account-store transactions with a shared lock.
- API keys, environment credentials, unrelated accounts, external-tool logins, and operating-system keychains are not included in this OAuth transfer.
- A dry run stages the ordinary session bundle but does not send credentials or change ownership.
- Both sides must support the hidden `cloud receive`, `cloud credentials`, and `cloud cancel-return` commands. The client checks all three before uploading or sending secrets. Local snapshot preparation overlaps these checks. Managed-host preflight can update the installed CLI, but does not restart an already-running cloud daemon.

`/local` (`jcode cloud return`) brings back the cloud account's refreshed tokens through the same separate channel. The local machine retains only a SHA-256 fingerprint of its departure credential envelope to detect an intervening local login or refresh. If the local account changed while the session was away, return refuses to overwrite it and restores cloud ownership. The fingerprint is removed after a successful return.

OAuth providers can rotate or revoke refresh tokens. Copying a login does not make two independently refreshing copies safe indefinitely. Avoid concurrently using the same account from local and cloud sessions. Store locks protect cooperating Jcode processes on one machine, not other machines, older Jcode binaries, or external credential editors.

## Requests during a turn

The TUI accepts `/cloud`, `/cloud <host>`, and `/local` while the agent is working. For `/cloud`, it prepares the destination host while the current turn continues, without snapshotting the live repository or copying the session or credentials. The actual handoff starts automatically once preparation succeeds and the current turn finishes, rather than requiring you to interrupt the agent and retry the command. `/local` waits for the same safe turn boundary. The queued request is local to that TUI instance. Closing the client before dispatch does not persist the request.

This is a deferred handoff, not live process migration. An in-flight model stream or tool process is not moved between machines. Waiting for the turn boundary keeps tool results and the session snapshot consistent. Background processes are still local-only and are described in the migration notice.

## Managed-host release compatibility

For the managed `jcode-cloud` host, a published client selects its own exact runtime release tag for any required upgrade. For example, client `v0.99.123` downloads `v0.99.123`, not a hardcoded old version or an unbounded `latest`. A newer installed stable release is kept when it already supports all three handoff commands. An older installation, or one lacking a required command, is replaced only after the selected release passes verification.

Development clients do not guess a published release or automatically downgrade the cloud host. They can use an already-capable installation. To explicitly select a compatible published release, set `JCODE_CLOUD_RELEASE` on the **local client** before starting the move:

```sh
JCODE_CLOUD_RELEASE=v0.99.123 jcode cloud move
```

The tag above is illustrative: choose an existing published release with all required handoff commands. The override must be an exact numeric tag, with an optional `v` prefix. It also overrides the download target for published clients. A newer capable installation is still retained. Missing assets, invalid tags, checksum mismatches, unexpected archive contents, or unsupported commands stop the move before secrets are sent.

Preflight uses the release's existing manylinux-compatible `jcode-linux-x86_64.tar.gz` and its `SHA256SUMS`. It validates the checksum and flat archive layout, then executes the staged wrapper to check its exact release version and all three commands. The wrapper, real executable, and bundled OpenSSL libraries remain together.

The VM's `~/.local/bin/jcode` and service launchers are preserved, including their host-specific environment. The shared `/opt/jcode-cloud/jcode` backend is switched with an atomic rename, and the **effective CLI launcher** is verified afterward. The previous backend is retained beside it as `jcode.previous.<release-directory>`. Failed post-switch verification restores that backend. If rollback itself fails, recovery files are retained and the move reports an error instead of claiming success. Installation needs noninteractive `sudo`; preflight never kills sessions or restarts the service. An existing daemon keeps its current executable until its normal next start.

Automatic upgrades apply only to the managed host. Other SSH targets or custom launchers must already provide compatible commands; their binaries are not automatically replaced.

## Transfer performance and recovery

Host preparation and local snapshot preparation run concurrently. SSH protocol
checks share one connection, and managed-host connections reuse a private SSH
control socket for up to 60 idle seconds instead of repeating authorization and
the SSH handshake for every operation. Custom transports retain their individual
protocol checks.

For an existing SSH checkout, the client searches up to 128 remote commits for a
locally known common ancestor. When possible, the repository bundle excludes that
history. New checkouts, unavailable probes, and repositories without a suitable
common ancestor use a self-contained bundle. The branch tip is always included
for compatibility with older receivers. The receiver verifies incremental bundle
prerequisites before restoring the checkout. Logs include elapsed times for each
major transfer stage and the bundle's byte count.

Unreturned cloud edits are never silently overwritten. If another session left
changes in the same cloud checkout, recover that session first with
`jcode cloud return --session <id>` from the local machine. The conflict error
identifies the earlier session when a single snapshot owner is recorded. This is
not a reason to reset or clean the cloud checkout: doing so could discard work.

## Validation

The process-level acceptance suite uses synthetic credentials, isolated `JCODE_HOME` directories, and a local transport instead of SSH. It exercises both providers, refresh-token return, expired access tokens, identity-less accounts, dry runs, conflicts, and secret redaction without accessing any real provider or account:

```sh
python3 tests/test_cloud_credentials.py /absolute/path/to/new/jcode
```

The auth transfer and account-store modules also have focused Rust tests for payload validation, permissions, symlinks, concurrent writers, and preservation of existing accounts.
