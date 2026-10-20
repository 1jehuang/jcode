# Cloud session handoff and OAuth accounts

`/cloud` (or `jcode cloud move`) copies the current session's **selected Jcode-owned OpenAI or Claude OAuth account** to the chosen SSH host. Choosing the destination and running the move authorizes that account copy. Use only a host you trust with the account's access and refresh tokens.

- The original account label is retained and the migrated session is pinned to it. A missing explicitly selected account is an error, not a reason to choose another login.
- Tokens travel in a separate SSH stdin/stdout exchange. They are not included in the session/repository archive, manifests, command arguments, progress messages, or diagnostic output. Remote credential-command stderr is deliberately discarded to prevent accidental token reflection.
- The destination store uses private permissions and atomic publication. Other accounts and the active selection are preserved. A conflicting identity is not overwritten. Current Jcode processes coordinate account-store transactions with a shared lock.
- API keys, environment credentials, unrelated accounts, external-tool logins, and operating-system keychains are not included in this OAuth transfer.
- A dry run stages the ordinary session bundle but does not send credentials or change ownership.
- Both sides must run a build with the hidden `cloud credentials` command. The client checks support before sending secrets. Updating only the client does not update an already-running cloud installation.

`/local` (`jcode cloud return`) brings back the cloud account's refreshed tokens through the same separate channel. The local machine retains only a SHA-256 fingerprint of its departure credential envelope to detect an intervening local login or refresh. If the local account changed while the session was away, return refuses to overwrite it and restores cloud ownership. The fingerprint is removed after a successful return.

OAuth providers can rotate or revoke refresh tokens. Copying a login does not make two independently refreshing copies safe indefinitely. Avoid concurrently using the same account from local and cloud sessions. Store locks protect cooperating Jcode processes on one machine, not other machines, older Jcode binaries, or external credential editors.

## Validation

The process-level acceptance suite uses synthetic credentials, isolated `JCODE_HOME` directories, and a local transport instead of SSH. It exercises both providers, refresh-token return, expired access tokens, identity-less accounts, dry runs, conflicts, and secret redaction without accessing any real provider or account:

```sh
python3 tests/test_cloud_credentials.py /absolute/path/to/new/jcode
```

The auth transfer and account-store modules also have focused Rust tests for payload validation, permissions, symlinks, concurrent writers, and preservation of existing accounts.
