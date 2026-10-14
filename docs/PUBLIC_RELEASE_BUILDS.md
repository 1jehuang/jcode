# Public release build routing

The tagged public release workflow (`.github/workflows/release.yml`) uses
`CARGO_PROFILE_RELEASE_OPT_LEVEL=3`. Local Cargo profiles are unchanged:
`release` remains level 1 and `selfdev` remains level 0, with existing
per-package overrides retained.

## justrust integration

Linux, macOS, and Windows compilation uses the pinned justrust client installed
by `scripts/install_justrust_ci.sh`. The installer selects the compiler host,
not the Jcode target (Windows ARM64 cross-compilation still uses the x64 client),
verifies the published archive checksum, validates the executable before replacing
an installation, and never logs in or installs from an unpinned source fallback.

These builds explicitly use `--local`: execution stays on the GitHub runner or
inside the existing manylinux2014 container. `JUSTRUST_TOOLCHAIN=system` preserves
the workflow's stable compiler, or nightly for the ARM64 emulated-TLS build.
Slots are disabled to preserve artifact paths used by packaging. FreeBSD retains
Cargo because there is no published native justrust client or hosted FreeBSD route.
The compatibility script selects justrust only when `JCODE_CI_BUILD=1`, leaving
ordinary local compatibility builds unchanged.

## Hosted migration gates

Inspection on October 14, 2026 found no justrust usage in the previous public
release workflow and no `JCODE_API_KEY` repository secret. Hosted compilation is
not yet a safe replacement for these release environments:

- Linux x86_64 assets intentionally build against glibc 2.17 in manylinux2014.
  Building on the hosted Ubuntu environment must not silently raise that ABI
  requirement. Hosted needs a validated compatible sysroot/container path first.
- Linux ARM64 rebuilds standard libraries with nightly and emulated TLS for
  Termux compatibility. Current optimized-build routing only accepts native
  Linux GNU targets, not this cross-target path from the x86_64 hosted fleet.
- Native Windows and macOS hosted routing remains disabled pending native worker,
  SDK, artifact-return, and runtime acceptance. FreeBSD has no hosted route.
- The hosted environment allowlist does not forward the workflow's
  `CARGO_PROFILE_RELEASE_OPT_LEVEL` override. A hosted migration must preserve
  level 3 explicitly, along with release metadata and the selected toolchain.

Before enabling hosted, validate the actual Jcode executable's ABI/architecture,
release metadata, artifact paths/checksums, and native smoke tests. Configure CI
account authentication via a repository secret only after the route is supported.
Keep signing and publication separate from compilation and do not send deployment
or signing credentials to build workers. No hosted speedup is claimed here.

## Recovery of an existing draft without moving its tag

For a failed tagged release, commit the reviewed source fix and recovery workflow
on `master`, then dispatch the workflow from `master` with the existing tag:

```bash
gh workflow run release.yml --ref master -f tag=v0.96.0 -f recovery=windows-listener
```

This is a full GitHub-runner rebuild of every platform, not a local asset upload
or a publication-gate override. Complete v0.96.0 before starting v0.97.0. The
workflow requires an existing draft and rejects recovery if a newer stable
version is already public. Both checks fail closed on API errors and are repeated
immediately before publication. Per-tag concurrency queues runs rather than
cancelling a build that may already have attached draft assets.

The only source repair is `windows-listener`, allowlisted for v0.96.0 and
v0.97.0. It adds a Windows-only mutable shadow binding in the companion server.
`scripts/apply_release_recovery.py` verifies the tag's package version, clean
tracked checkout, canonical original file SHA256, exact replacement and repaired
SHA256, and the committed repair source at
`d4f4c8c8d25c9f8e062773683adcc0c19cf26144`. Windows CRLF checkout is normalized
only for comparison against canonical Git bytes. Unknown tags, arbitrary patches,
wrong HEAD, unrelated edits and unexpected source content are rejected. The fix
commit must be an ancestor of the workflow commit.

Every downstream job checks out the same resolved tag commit, preserving HEAD,
Cargo version and tag identity. The repair script and release-only justrust
installer are loaded from the immutable workflow commit, never a moving branch
tip. Older tags lack that installer, so it is materialized with executable
permissions and its digest recorded. The tag's historical Linux compatibility
helper remains in use. A recovery therefore deliberately produces patched tag
source, not the exact tagged tree. Release notes disclose this distinction and
`release-provenance.json` records tag/workflow/fix commits, file hashes, release
helper hashes and the Actions run. Provenance is generated outside the checkout
until final packaging, so normal `recovery=none` builds remain clean.

Windows launch and installer validation, configured signing policy and signature
verification, the complete nine-asset platform gate, checksum generation and
publication all remain in the usual workflow. The existing
`WINDOWS_SIGNING_REQUIRED=false` policy, if configured, still explicitly permits
unsigned Windows binaries. Recovery does not change that setting. Provenance is
uploaded and included in `SHA256SUMS` before the draft becomes public. Missing
assets keep it draft. Recovery skips bulk issue closure because labels may now
refer to fixes newer than the recovered tag.

`.github/workflows/windows-companion.yml` adds fast, non-optional companion
checks on both Windows targets for relevant PRs. `.github/workflows/release-recovery.yml`
fetches full tag history and runs the recovery regressions. Run locally with:

```bash
python3 tests/test_release_recovery.py
actionlint -ignore 'SC2129' .github/workflows/release.yml .github/workflows/windows-companion.yml .github/workflows/release-recovery.yml
```

The SC2129 suppression is the pre-existing AUR redirect style warning. Recovery
tests exercise both actual tag trees in disposable clones, genuine CRLF checkout,
clean no-repair metadata, helper executability and provenance, invalid input and
dirty-source rejection. A controlled `gh` stub tests draft/newer-release/API-error
guards. These are not substitutes for successful remote platform builds.

## Temporary Windows measurement mitigation

The v0.97.0 Windows x64 log showed the pinned justrust alpha.17 client aborting
with a 30,064,771,072-byte allocation request before emitting compiler results.
Inspection confirmed `JUSTRUST_DISABLE=1` takes its early `exec_cargo` path,
bypassing measurement/shims while preserving Cargo arguments and exit status.
This setting is scoped to the Windows release-build job and Windows companion
compile job only. Both still use the pinned CLI, `--local`, system toolchains and
deterministic paths. Native Windows hosted routing is unsupported, so this is
not a hosted billing bypass. Linux and macOS measurement remain enabled. No
compile, smoke, signing, checksum or complete-platform publication gate is
bypassed. Remove the setting after a pinned client fixes the Windows crash and
passes native acceptance. Successful remote rebuilds are still required.

## Validation

Run `python3 tests/test_install_justrust_ci.py`, `bash -n` on both installer and
compatibility scripts, and `actionlint .github/workflows/release.yml` (the existing
SC2129 style warning in a separate publication step is unrelated). A Linux smoke
check installed the actual alpha.17 published client and built/launched an isolated
release fixture at level 3 and a selfdev fixture at level 0. This does not replace
full GitHub Actions acceptance on the other operating systems or in Docker.
