# Releasing jcode

Official releases have two gates: **successful CI for the exact tagged commit** and **a complete platform asset set before publication**. Local preparation can save build time, but it does not bypass either gate. `.github/workflows/release.yml` owns final publication.

## Normal release procedure

1. Prepare and commit the version bump in `Cargo.toml`, `Cargo.lock`, and the changelog. Commit any source changes before preparing a cached fast-release binary.
2. Push the release commit to `master` (or `main` where applicable) **without pushing a release tag**.
3. Wait for `.github/workflows/ci.yml` to succeed for that exact commit. A green parent, pull request run, different SHA, or older successful attempt is not sufficient.
4. Run the release helper. Its fail-fast preflight checks CI before creating or pushing the tag:

   ```bash
   git push origin master
   python3 scripts/check_release_ci.py "$(git rev-parse HEAD)" \
     --repo 1jehuang/jcode --timeout 0
   scripts/quick-release.sh --remote v0.98.1
   ```

   The explicit Python check is optional because the helper repeats it. If CI is pending, missing, failed, cancelled, or cannot be queried, the helper exits without pushing the tag. Wait for successful CI and retry. It also rejects a tag version that differs from the committed `Cargo.toml` package version, conflicting tags, failed remote lookups, and HEAD changes during preparation/preflight.
5. Watch the tag-triggered release workflow. Do not announce or manually publish a partial draft. Publication waits for all required platform builds, Windows signing, checksums, and provenance.

   Use `scripts/watch_release_run.sh <run-id>` to monitor it to completion. Its default four-hour budget covers the 150-minute Windows build timeout plus finalization and runner queues. A successful v0.97 recovery run took about 71 minutes, so a short wait is not evidence that a release is stuck. Every completed non-success conclusion is reported immediately.

Use the helper instead of manually pushing `v*` tags. Git itself cannot enforce this client-side preflight, so the workflow independently repeats the exact-SHA CI check before building or creating its draft. A tag pushed by hand does not bypass that workflow gate.

### Exact-SHA CI evidence

`scripts/check_release_ci.py` selects the latest exact-SHA `push` run of `ci.yml` on `main`/`master`, checks the latest attempt, and requires the run and these mandatory jobs to have completed successfully:

- Quality Guardrails
- Build & Test (ubuntu-latest)
- Build & Test (macos-latest)
- Build & Test (windows-latest)
- Windows ARM64 Release Configuration

It checks run identity, paginated job results, and confirms the attempt did not change during verification. Missing or skipped mandatory jobs, ambiguous evidence, API failures, and unsuccessful conclusions cannot authorize a release. The helper uses `--timeout 0` to reject immediately rather than tagging while CI is pending. The workflow uses a bounded wait for its own independent gate.

### Why the pre-tag gate is necessary

On October 14, 2026, release commit `5205cdf6fde97e07775b9a491bdd4153ab25f095` was created at **18:44:10 UTC**, and `v0.98.0` was tagged at **18:44:12 UTC**. Its exact-SHA CI run `38617673934` was not created until **18:44:13 UTC** and ultimately failed. The release workflow run `38617680965` started at **18:44:16 UTC** and failed as well.

The helper previously allowed immediate tag pushes, relying only on the later workflow publication gate. That prevented successful normal publication but did not prevent premature tagging. All publishing modes now share the successful exact-SHA preflight before tag creation/push, including retries with an existing tag. Existing tags are not moved or deleted by this procedure.

## Release helper modes

| Mode | Local work | Publication |
|------|------------|-------------|
| `--remote` / `--ci-only` | None | CI builds and publishes the complete release |
| Default | Build Linux x86_64 and macOS aarch64 in parallel | Stage on a draft, then CI completes and publishes |
| `--prepare-fast` | Refresh and record the Linux selfdev binary before the metadata commit | No tag or release |
| `--fast-local` / `--fast` | Validate and package that prepared Linux binary | Stage on a draft, then CI replaces it with the portable build |
| `--prepare-fast-macos` | Refresh and record the osxcross macOS aarch64 binary before the metadata commit | No tag or release |
| `--fast-macos-local` / `--fast-macos` | Validate, sign, and package that prepared macOS binary | Stage on a draft, then CI replaces it with the signoff build |
| `--dry-run` with a local packaging mode | Build/validate/package only | No CI preflight, tag, upload, or publication |

```bash
scripts/quick-release.sh --remote v0.98.1 "Release title"
scripts/quick-release.sh v0.98.1
scripts/quick-release.sh --dry-run v0.98.1
```

Fast modes require the recorded binary hash to be unchanged and the preparation commit to be the release commit's parent. The release commit may modify only `Cargo.toml`, `Cargo.lock`, and `changelog/`. Push that metadata commit and wait for its own CI before running the packaging mode. The staged Linux selfdev binary targets the build host's runtime baseline until CI replaces it.

All local preparation/build modes require a clean working tree. Remote mode tags only committed HEAD and warns that working-tree changes are excluded. Non-dry-run release modes require authenticated `gh` and Python 3.11 or newer, including remote mode. The repository used for the preflight is resolved from `origin`, not an unrelated `GH_REPO` default. Local staging refuses to overwrite an already-public release.

The fast paths no longer make single-platform releases public immediately. They leave publication to the complete-platform workflow, just like the default path. Build durations depend on cache warmth and runner availability. Exact-SHA CI must finish before tag push, then release builds and signing must finish before publication.

## CI release workflow

A pushed `v*` tag triggers `.github/workflows/release.yml` in the canonical `1jehuang/jcode` repository:

```text
Tag push (after successful exact-SHA main/master CI)
    |
    +--> create-release
    |      Validate immutable tag/source identity
    |      Recheck exact-SHA CI and mandatory jobs
    |      Verify frozen dependency resolution (Cargo.lock)
    |      Create/update hidden draft and release notes
    |
    +--> build-linux-macos (parallel platform matrix)
    |      Linux x86_64 and aarch64
    |      macOS aarch64 and x86_64
    |
    +--> build-windows --> publish-windows
    |      Windows x86_64 and ARM64 builds
    |      Azure Artifact Signing and Authenticode verification
    |      Upload signed executables and archives to draft
    |
    +--> build-freebsd
    |      FreeBSD x86_64
    |
    +--> release (after platform jobs reach terminal states)
           Require the complete platform asset set
           Attach all final assets
           Generate/upload SHA256SUMS and release-provenance.json
           Publish the completed release
           Download/verify every public asset, checksum, and provenance
           Install the public Linux archive in an isolated home and verify its version
           Queue Discord announcement
           Update Homebrew and AUR when credentials are configured
```

### Required assets

Publication requires all nine platform files:

- `jcode-linux-x86_64.tar.gz`
- `jcode-linux-aarch64.tar.gz`
- `jcode-macos-aarch64.tar.gz`
- `jcode-macos-x86_64.tar.gz`
- `jcode-windows-x86_64.exe` and `jcode-windows-x86_64.tar.gz`
- `jcode-windows-aarch64.exe` and `jcode-windows-aarch64.tar.gz`
- `jcode-freebsd-x86_64.tar.gz`

A missing platform asset fails finalization and keeps a new release as a draft. Platforms build independently, but **they are not independently publishable**. Successful platforms alone are not enough. `SHA256SUMS` covers the final archives, Windows executables, and provenance file.

After making the complete release public, the finalizer downloads the actual public assets and verifies their hashes and exact-SHA unpatched provenance before announcements or package-manager updates. Repeat that verification when checking a release manually:

```bash
python3 scripts/verify_public_release.py v0.98.1 --repo 1jehuang/jcode \
  --sha "$(git rev-parse 'refs/tags/v0.98.1^{commit}')"
```

The verifier requires all nine platform files plus `SHA256SUMS` and `release-provenance.json`, nonempty uploaded assets, a checksum manifest covering exactly the published files, and provenance matching the expected immutable tag commit. It never executes or extracts downloaded binaries. Use `--directory <new-directory>` to retain the verified downloads. A failed post-publication verification blocks the workflow's downstream announcement/package steps, but does not automatically make an already-public release private.

Homebrew needs the four Linux/macOS archives. AUR needs Linux x86_64. These package-manager checks do not weaken the complete-platform publication gate. Updates run after publication and also require their configured deployment credentials.

### Windows signing prerequisites

The one-time setup is documented in [docs/WINDOWS.md](docs/WINDOWS.md#enable-authenticode-signing). The release repository needs:

- Secrets: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`
- Variables: `WINDOWS_SIGNING_ENDPOINT`, `WINDOWS_SIGNING_ACCOUNT`, `WINDOWS_SIGNING_CERTIFICATE_PROFILE`
- Emergency override only: `WINDOWS_SIGNING_REQUIRED=false`, which is not suitable for an official signed Windows build

Before announcing Defender or SmartScreen remediation, download both public Windows executables and confirm `Get-AuthenticodeSignature` reports `Valid`.

At v0.98.1 publication, the repository had `WINDOWS_SIGNING_REQUIRED=false` and no Azure signing credentials or account configuration. Its Windows executables are unsigned, not Authenticode-verified. Release notes now disclose the actual signing mode, including this override. To enable signed future releases, an administrator must provision the Azure identity and signing account/profile listed above and remove the override. Do not claim that a successful packaging job proves signing occurred.

### Existing-tag recovery

Normal retries still require successful CI on the existing tag's exact SHA. A newer green master commit does not authorize an older failed tag. Reviewed source recovery is a separate, explicitly constrained workflow-dispatch path with provenance, not a bypass available in `quick-release.sh`. Do not retag an existing version or broaden recovery permissions as part of a routine release.

For a **workflow-only defect** with an unpublished draft and already-green exact tagged source, commit and review the workflow fix on `master`, then dispatch the corrected workflow against the unchanged tag with `recovery=none`:

```bash
gh workflow run release.yml --ref master -f tag=v0.98.1 -f recovery=none
```

This rebuilds all platforms from the original tag, repeats the exact-tag-SHA CI check, and records the separate workflow commit in provenance. It refuses a public release or a version older than an already-public successor. A rerun of the old failed run uses its old workflow definition, so it cannot pick up a committed workflow fix. Wait for signing, complete assets, public downloads, and installer checks before calling the retry successful.

The optional `recovery-preflight` job is intentionally skipped for unpatched releases. Every downstream job that must still run needs an explicit status-check expression. In particular, Windows signing uses `always()` plus successful direct dependency results, preventing a skipped recovery ancestor from suppressing signing without allowing failed Windows builds through.

## Local cross-compilation prerequisites

The default local mode needs Docker for the portable Linux build and osxcross for macOS, plus `cargo`, `file`, and `rcodesign`. The current osxcross setup uses the macOS 14.5 SDK at `~/.osxcross/`, with the `aarch64-apple-darwin23.5-clang` linker. Install the Rust `aarch64-apple-darwin` target and configure `~/.cargo/config.toml`:

```toml
[target.aarch64-apple-darwin]
linker = "aarch64-apple-darwin23.5-clang"

[env]
CC_aarch64_apple_darwin = "aarch64-apple-darwin23.5-clang"
CXX_aarch64_apple_darwin = "aarch64-apple-darwin23.5-clang++"
```

macOS binaries are ad-hoc signed with the stable identifier `com.jcode.cli`, using `codesign` when available and `rcodesign` otherwise. System framework dependencies such as AppKit and Security require real macOS SDK headers/framework stubs, which osxcross provides.

## Release gate regression checks

```bash
bash -n scripts/quick-release.sh
python3 scripts/test_quick_release.py
python3 tests/test_release_ci.py
python3 tests/test_verify_public_release.py
```

The quick-release tests exercise the real remote and standard-mode script with disposable Git repositories and bare remotes plus a simulated GitHub API. Standard-mode binary builders and signing are stubbed, while CI gating, tagging, packaging, and draft-only staging run through the real script. They cover successful exact-SHA tagging, non-green/missing/wrong-SHA evidence, existing/conflicting tags, API/remote failures, and concurrent HEAD/tag changes without writing to GitHub. Helper unit tests cover latest-run/attempt selection, mandatory jobs, and bounded waiting.
