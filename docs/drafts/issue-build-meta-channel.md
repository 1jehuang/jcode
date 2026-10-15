# [Bug] build-meta: channel segment dropped from build-time and runtime version env vars

> **Draft — not yet filed. Phinbox broken 2026-10-14T07:00Z (operator signal: "phinbox broken must go wo for now"), so the file-phinbox gate is suspended. Re-check phinbox health before filing.**
> Targeted repo: `KooshaPari/Jcode` (fork of `1jehuang/jcode`).
> Note: this issue was discovered while implementing the fix. The fix
> itself is in branch `fix/build-meta-channel-preserve` on the same
> fork; if the issue is filed, the PR can be linked from it.

## Summary

`crates/jcode-build-meta/` had channel preservation broken in two
places, on opposite sides of the build/runtime boundary:

1. **Build-script side (`build.rs`)**: when
   `JCODE_BUILD_SEMVER=0.91.0-k0.1.2-channel` is set, the env vars
   emitted to the compiled binary lose the `-k0.1.2-channel` suffix.
   The build script reconstructs the version triple with
   `format!("{}.{}.{}", major, minor, patch)`, which silently
   discards the channel.
2. **Runtime side (`src/lib.rs`)**: `parse_release_semver()` (which
   the `JCODE_RUNTIME_RELEASE_SEMVER` fast-release wrapper uses to
   override the reported identity) had the same bug. Worse, its
   patch parse fails outright on `-k0.1.2-channel` because the patch
   field is read as `u32`, so the override silently falls back to
   the compile-time value.

## Observed (build-time, before fix)

With `JCODE_BUILD_SEMVER=0.91.0-k0.1.2-channel`:

| Env var | Before | After |
|---|---|---|
| `JCODE_VERSION` | `v0.91.0-dev (git_hash)` | `v0.91.0-k0.1.2-channel-dev (git_hash)` |
| `JCODE_SEMVER` | `0.91.0` | `0.91.0-k0.1.2-channel` |
| `JCODE_BASE_SEMVER` | `0.91.0` | `0.0.0-k0.1.2-channel` |
| `JCODE_UPDATE_SEMVER` | `0.91.0` | `0.91.0-k0.1.2-channel` |

Confirmed by `cargo build -vv`.

## Observed (runtime, before fix)

Setting `JCODE_RUNTIME_RELEASE_SEMVER=0.91.0-k0.1.2-channel` and
calling any of `version()`, `semver()`, `base_semver()`,
`update_semver()`, `pkg_version()` returns the compile-time value
without the channel; the override is silently dropped.

## Impact

1. The channel flag is the safety guard against cross-channel upgrades.
   If the binary reports a channel-less version, the in-app updater
   can mis-classify a `dev`-channel binary as a `stable`-channel one
   and silently install the wrong channel.
2. `--version` is the verification step `AGENTS.md` prescribes. After
   this fix, that step actually reflects what was built (or what the
   fast-release wrapper says was built).
3. Same class of bug as #799 (commit hash), but on a different
   dimension (channel).

## Reproduction

### Build-time

```bash
git clone https://github.com/KooshaPari/Jcode
cd jcode
JCODE_BUILD_SEMVER=0.91.0-k0.1.2-channel cargo build -vv 2>&1 \
  | grep -E "JCODE_(VERSION|SEMVER|BASE_SEMVER|UPDATE_SEMVER)="
```

Before the fix: all four env vars are channel-less. After the fix: all
four env vars carry the `-k0.1.2-channel` suffix.

### Runtime

```bash
JCODE_RUNTIME_RELEASE_SEMVER=0.91.0-k0.1.2-channel \
  jcode --version
```

Before the fix: `--version` reports the compile-time value (which is
itself channel-less for the same reason). After the fix: `--version`
reports `v0.91.0-k0.1.2-channel (hsh)`.

## Expected behavior

A binary built with `JCODE_BUILD_SEMVER=...channel` should report
that channel in every version-shaped env var. A binary started with
`JCODE_RUNTIME_RELEASE_SEMVER=...channel` should report the channel
in all five runtime accessor functions (`version`, `semver`,
`base_semver`, `update_semver`, `pkg_version`). The behavior is
unchanged when no channel is present in the input.

## Proposed fix

- Build script: replace the channel-discarding `format!()` with a
  helper that joins the components and conditionally appends the
  channel segment. Switch the override filter from `parse_semver` to
  `parse_semver_with_channel` so the override path accepts
  channel-bearing values.
- Runtime: extend `parse_release_semver` to split on the first `-`
  (same convention as the build-script side), parse
  `major.minor.patch` from the version part, and carry the channel
  through unchanged in the returned string.

The fix is implemented in
`https://github.com/KooshaPari/Jcode/pull/<TBD>` (commits
`94414603f` and `f66528041` on branch
`fix/build-meta-channel-preserve`).

## Related

- **#1257** — umbrella: "Self-dev upgrades should expose durable
  build identity across client and shared-server channels"
- **#799** (closed) — prior art: "install_release.sh installs
  binaries that misreport their own commit"
- **#1796** — fmt blindness, separately fixed in
  `https://github.com/KooshaPari/Jcode/pull/<TBD>` (commit
  `2c2c35c3b` on branch `triage/2026-10-13-issues`).
