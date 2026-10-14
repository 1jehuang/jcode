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

## Validation

Run `python3 tests/test_install_justrust_ci.py`, `bash -n` on both installer and
compatibility scripts, and `actionlint .github/workflows/release.yml` (the existing
SC2129 style warning in a separate publication step is unrelated). A Linux smoke
check installed the actual alpha.17 published client and built/launched an isolated
release fixture at level 3 and a selfdev fixture at level 0. This does not replace
full GitHub Actions acceptance on the other operating systems or in Docker.
