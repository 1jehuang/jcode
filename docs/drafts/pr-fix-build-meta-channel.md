# PR: fix(build-meta): preserve channel segment in build-time and runtime version env vars

> **NOTE — issue context (operator review needed)**
>
> **Phinbox broken 2026-10-14T07:00Z (operator signal: "phinbox broken must go wo for now")** — the `gh pr create` and `gh issue create` gates are currently non-functional. PR is ready to file but the file-phinbox gate is suspended. Re-check phinbox health before filing.
>
> The first commit's `tx-task: #37` trailer is wrong: actual #37 in
> this repo is "Self-dev customizations: integrate records into update
> flow…" (unrelated to channel preservation). The closest umbrella is
> **#1257** ("Self-dev upgrades should expose durable build identity
> across client and shared-server channels"), which references #799 as
> the prior art. A focused, narrow new issue for the build-time half
> of this bug has not been filed yet (phinbox-gated); the runtime-half
> was added in the second commit and shares the same umbrella. Drafts
> for both are at `docs/drafts/issue-build-meta-channel.md`.
>
> The second commit's `tx-task: #1257` is correct.

## What this fixes

`crates/jcode-build-meta/` had channel preservation broken in two
places, on opposite sides of the build/runtime boundary:

### 1. Build-script side (`build.rs`, commit `94414603f`)

The build script was reconstructing the version triple with
`format!("{}.{}.{}", major, minor, patch)`, which silently discarded
the channel segment. Concretely, with
`JCODE_BUILD_SEMVER=0.91.0-k0.1.2-channel`:

| Env var | Before | After |
|---|---|---|
| `JCODE_VERSION` | `v0.91.0-dev (git_hash)` | `v0.91.0-k0.1.2-channel-dev (git_hash)` |
| `JCODE_BASE_SEMVER` | `0.91.0` | `0.0.0-k0.1.2-channel` |
| `JCODE_UPDATE_SEMVER` | `0.91.0` | `0.91.0-k0.1.2-channel` |

A secondary bug in `explicit_build_semver_override()` was that the
override filter used the OLD `parse_semver`, which rejects
channel-bearing values. The override was therefore being silently
dropped before it could even reach the format code. Switched the
filter to `parse_semver_with_channel`.

### 2. Runtime side (`src/lib.rs`, commit `f66528041`)

`parse_release_semver()` (exercised by the `JCODE_RUNTIME_RELEASE_SEMVER`
fast-release wrapper) had the same channel-dropping bug: it split on
`.` only, parsed `major.minor.patch` as `u32`, and reconstructed with
`format!("{major}.{minor}.{patch}")`. For `0.91.0-k0.1.2-channel` the
patch parse would even fail (the `-k0.1.2` part), so the override
silently fell back to the compile-time value. After the fix the
override carries the channel through to `version()`, `semver()`,
`base_semver()`, `update_semver()`, and `pkg_version()`.

## Why this matters

1. The channel flag is the safety guard against cross-channel upgrades.
   If the binary reports a channel-less version, the in-app updater
   can mis-classify a `dev`-channel binary as a `stable`-channel one
   and silently install the wrong channel.
2. `--version` is the verification step `AGENTS.md` prescribes. After
   this fix, that step actually reflects what was built (or what the
   fast-release wrapper says was built).
3. Same class of bug as #799 (commit hash), but on a different
   dimension (channel).

## Verification

### Build-time (`cargo build -vv` with `JCODE_BUILD_SEMVER=0.91.0-k0.1.2-channel`)

- `JCODE_VERSION=v0.91.0-k0.1.2-channel-dev (324641056, dirty)`
- `JCODE_SEMVER=0.91.0-k0.1.2-channel`
- `JCODE_BASE_SEMVER=0.0.0-k0.1.2-channel`
- `JCODE_UPDATE_SEMVER=0.91.0-k0.1.2-channel`

### No-channel regression

All four env vars return `0.0.0` / `v0.0.0-dev (hsh, dirty)` — unchanged.

### Runtime tests

```
running 5 tests
test tests::runtime_release_semver_rejects_malformed ... ok
test tests::runtime_release_semver_preserves_simple_channel ... ok
test tests::runtime_release_semver_preserves_dashed_channel ... ok
test tests::runtime_release_semver_strict_three_numeric_components ... ok
test tests::runtime_release_semver_treats_empty_channel_as_no_channel ... ok
test result: ok. 5 passed; 0 failed
```

### Build-script tests (existing, in `build.rs`)

`parse_semver_with_channel_*` and `channel_suffix_*` — all pass.

### `cargo fmt -p jcode-build-meta` clean. `cargo check -p jcode-build-meta` clean.

## Related issues

- **#1257** — umbrella: "Self-dev upgrades should expose durable
  build identity across client and shared-server channels"
- **#799** (closed) — prior art: "install_release.sh installs
  binaries that misreport their own commit". Same class of bug,
  different dimension (commit hash vs channel).
- **#1796** — fmt blindness, separately fixed in
  `https://github.com/KooshaPari/Jcode/pull/<TBD>` (commit
  `2c2c35c3b` on branch `triage/2026-10-13-issues`).

## Commits in this PR

1. `94414603f` — build-script side (channel preservation in env vars)
2. `f66528041` — runtime side (channel preservation in
   `parse_release_semver`)

tx-agent: jcode
tx-validated: test
tx-scope: jcode-build-meta
tx-intent: restore channel preservation in build-time and runtime version env vars
tx-parent: 324641056
