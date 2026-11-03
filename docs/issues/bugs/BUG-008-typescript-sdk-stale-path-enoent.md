# [BUG-008] TypeScript SDK tests fail with 6 ENOENT (stale path)

**ID:** BUG-008
**Category:** bug
**Severity:** medium
**File:** sdk/typescript/ (test harness)
**Status:** open

## Description
The TypeScript SDK job fails with 6 ENOENT errors attributed to a stale path. The SDK test harness
resolves a path (likely a prebuilt binary or generated artifact) that no longer exists after the
decollision rename/build changes. CI notes this is a checkout artifact issue rather than a
behavioral parity failure, but it still blocks the job.

## Evidence
- CI job "TypeScript SDK :: failure" — 6 ENOENT stale-path failures.
- SDK layout:
  ```
  sdk/typescript/package.json
  sdk/npm/{win32-arm64,linux-arm64,darwin-x64,win32-x64,darwin-arm64,linux-x64}/package.json
  ```

## Suggested fix
Find the ENOENT path in the SDK job logs, update the resolver to the post-rename location (or make
it discover the binary instead of hardcoding `jcode`), and add an existence check with a helpful
message. Re-run `pnpm --dir sdk/typescript test`.

## Effort
- S (< 30m)
