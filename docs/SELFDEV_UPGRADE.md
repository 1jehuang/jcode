# Self-dev upgrades and rollback

Self-dev changes are source changes, not mutations that should be kept only in
the installed executable. Treat the source checkout and its Git commit as the
authoritative copy.

## Upgrade-safe workflow

1. Work in a dedicated branch or worktree.
2. Commit and push the change before updating jcode.
3. Record the source commit with `git rev-parse HEAD` and the binary identity
   with `jcode version --json`.
4. Fetch the upstream branch and create a fresh upgrade worktree from it.
5. Merge or rebase the self-dev branch in that worktree.
6. Run formatting, compile, focused tests, and public smoke tests.
7. Build with `jcode self-dev --build` and reload only after the checks pass.

The machine-readable version report includes the source hash, runtime version,
build channel, and `external_provider_protocol_version`. Consumers should reject
an incompatible provider protocol before starting a turn.

## Build channels

Jcode keeps immutable binaries under `~/.jcode/builds/versions/` and uses
channel links for `current`, `stable`, and `shared-server`. Self-dev publishes
the tested source build to `current` while leaving `stable` unchanged. The
shared server is advanced only when it is tracking stable, or when explicitly
promoted by the self-dev reload flow.

Never overwrite an immutable version in place. A failed reload records a
pending activation and restores the previous `current` and `shared-server`
versions. Keep the previous version until the replacement has started, passed
its compatibility checks, and resumed the session.

## What an application update preserves

Application updates replace binaries, not user state. The following must remain
outside the source checkout and binary channels:

- provider credentials and auth state
- `config.toml` and MCP configuration
- sessions and restart snapshots
- build manifest and pending activation state

If a source change requires a data migration, add a versioned migration before
changing the persisted shape. A restart or reload must fail clearly rather than
silently discarding state.

## Rollback

Use the previous immutable version recorded in the build manifest. Do not reset
the source checkout or delete the previous binary while a server may still be
using it. After rollback, rerun `jcode version --json`, reconnect the client,
and verify session rendering and provider selection before trying the update
again.

