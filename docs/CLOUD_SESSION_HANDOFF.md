# Cloud session handoff and OAuth accounts

`/cloud` (or `jcode cloud move`) copies **all stored Jcode-owned OpenAI and Claude OAuth accounts** to the chosen SSH host, not just the current session's selected account. Choosing the destination and running the move authorizes those account copies. Use only a host you trust with every account's access and refresh tokens.

- The original account label is retained and the migrated session is pinned to it. A missing explicitly selected account is an error, not a reason to choose another login.
- Tokens travel in a separate SSH stdin/stdout exchange. They are not included in the session/repository archive, manifests, command arguments, progress messages, or diagnostic output. Remote credential-command stderr is deliberately discarded to prevent accidental token reflection.
- The destination store uses private permissions and atomic publication. Other accounts and the active selection are preserved. A conflicting identity is not overwritten. Current Jcode processes coordinate account-store transactions with a shared lock.
- The account router's other credential sources also move, one way, over a second private SSH stdin exchange (`cloud router-credentials`), so model routing and auto-switch fallback behave the same on the cloud host. The set is an explicit allowlist, never a directory dump:
  - Jcode home: `account-pool.json` (auto-switch order, merged so cloud-only entries survive), `gemini_oauth.json`, `antigravity_oauth.json`, `muse_subscription.json`.
  - Jcode config dir: every provider API-key env file the router loads (all OpenAI-compatible catalog and Muse profiles, `openai.env`, `anthropic.env`, `openrouter.env`, `gemini.env`, `azure-openai.env`, `bedrock.env`, `cursor.env`, `jcode-subscription.env`) plus the `env_file` of each configured named provider, filtered by `is_safe_env_file_name`. Only files that exist are sent. Source wins for each file. Destination-only files are untouched.
  - Files are validated as a whole bundle before any write, published atomically with 0600 permissions in 0700 directories, and never follow destination symlinks. They are not brought back on `/local`.
  - After import the remote command sends `notify_auth_changed` to a running cloud daemon, so API-key providers initialize without a restart.
- Deliberately excluded: external-tool logins (Codex CLI `~/.codex/auth.json`, Claude Code, GitHub Copilot, Gemini CLI, Grok, OpenCode/pi/OpenClaw/Hermes), `google_oauth.json`/`google_credentials.json` (Gmail/Calendar tools, not model routing), operating-system keychains, AWS profiles, and process environment variables. Jcode only reads external logins after a per-path trust decision on that machine, and their owning tools refresh and rotate them.
- A dry run stages the ordinary session bundle but does not send credentials or change ownership.
- Both sides must support the hidden `cloud receive`, `cloud credentials`, `cloud router-credentials`, and `cloud cancel-return` commands. The client checks all four before uploading or sending secrets. Local snapshot preparation overlaps these checks. Managed-host preflight can update the installed CLI, but does not restart an already-running cloud daemon.

`/local` (`jcode cloud return`) brings back refreshed tokens for every account in the local departure record through the same separate channel. The local machine retains only SHA-256 fingerprints of departure credential envelopes to detect an intervening local login or refresh. If a local account changed while the session was away, return refuses to overwrite it and restores cloud ownership. The fingerprints are removed after a successful return. Accounts added only on the cloud host are not imported implicitly.

OAuth providers can rotate or revoke refresh tokens. Copying a login does not make two independently refreshing copies safe indefinitely. Avoid concurrently using the same account from local and cloud sessions. Store locks protect cooperating Jcode processes on one machine, not other machines, older Jcode binaries, or external credential editors.

## Requests during a turn

The TUI accepts `/cloud`, `/cloud <host>`, and `/local` while the agent is working. For `/cloud`, it prepares the destination host while the current turn continues, without snapshotting the live repository or copying the session or credentials. The actual handoff starts automatically once preparation succeeds and the current turn finishes, rather than requiring you to interrupt the agent and retry the command. `/local` waits for the same safe turn boundary. The queued request is local to that TUI instance. Closing the client before dispatch does not persist the request.

Use `/cloud status` to inspect the pending request, `/cloud cancel` (or `/local cancel`) to cancel it, or submit another handoff command to replace its destination. Cancellation prevents the handoff but does not stop host preparation that is already running. Once the actual transfer starts, it is allowed to finish rather than being killed halfway through.

Unsent prompts, queued commands, and attachments are not part of the persisted session. The TUI refuses a handoff that would drop them. If you create new unsent work or switch sessions while a handoff is pending, it cancels the pending transfer rather than discarding that state. Send or save that work before retrying.

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
