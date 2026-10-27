# Native artifact handoff validation

`native-handoff.yml` is an opt-in experiment, **not a release publisher**. It does
not modify `release.yml`, create tags, upload release assets, or bypass the
release source / exact-commit CI gates. Windows native builds, Azure OIDC
Artifact Signing, and the release draft/publish path remain unchanged.

## Run it

Run the reviewed workflow from canonical `1jehuang/jcode` **master**. There is no
source-ref input: GitHub's dispatch SHA is the exact source checked out by every
job. A branch/tag/fork dispatch cannot run the producer. After these files reach
master, the following is the safe, **no-secret** native macOS validation run:

```bash
gh workflow run native-handoff.yml --repo 1jehuang/jcode --ref master \
  -f lane=macos-arm64 -f sign_macos=false
gh run list --repo 1jehuang/jcode --workflow native-handoff.yml --limit 5
gh run watch RUN_ID --repo 1jehuang/jcode --exit-status
gh run download RUN_ID --repo 1jehuang/jcode --dir native-handoff-results
```

Hosted ARM validation requires repository secret `JUSTRUST_API_KEY` for an
explicitly authorized hosted account with build credits. This uploads tracked
source to the hosted compiler and consumes its normal credits. Do not dispatch
it with another tenant's key or expect a no-secrets build:

```bash
gh workflow run native-handoff.yml --repo 1jehuang/jcode --ref master \
  -f lane=linux-arm64 -f sign_macos=false
```

The producer builds on `ubuntu-24.04` x86_64 using hosted justrust. The native
consumer runs on `ubuntu-24.04-arm`, not QEMU. GNU ARM64 retains target-only
`-Ztls-model=emulated` plus `-Zbuild-std=std,panic_abort`; host build scripts must
not receive emulated TLS. The pinned justrust client manages checksum-pinned
GNU cross-tools and its Cortex-A53 linker workaround. Its current sysroot is
Ubuntu 24.04 / glibc 2.39. This lane is **not evidence of compatibility with older
glibc, Android/Termux, or every release platform** and is not a default-release
switch.

## Pins and evidence

Both lanes use `nightly-2026-10-08`, `--locked --release --bin jcode`, default
features, opt-level 3, and no incremental compilation. Linux adds the rebuilt
standard-library flag above and `--features linux-compat-vendored-openssl` so
OpenSSL is built for the cross target rather than discovered from the host.
Default features remain enabled on both lanes; macOS adds no extra feature.
The source SHA, Cargo.lock digest, target,
producer kind, command, flags, toolchain, client revision, binary digest, and
build-report digest are sealed in `provenance.json`. Native smoke checks run
`--version`, `--help`, and `run --help` with isolated HOME/config, stripped
credentials, checked exit status, output markers, and 30-second timeouts.
These commands do not request a network operation or start a daemon. The harness
is not a network sandbox against a malicious executable.

The published alpha.19 client predates the GNU ARM64 contract. Rather than
pretend it supports that contract, the workflow bootstraps with the existing
pinned CI installer and locally builds client revision
`e020bad4c407b573b74cc0aa9ab7d5a90d98018a`. This extra bootstrap time is deliberate;
replace it with a reviewed checksum-pinned published client when available.
Only the Jcode ARM build goes to hosted infrastructure. Its successful JSON
receipt and local justrust metadata must report a remote build on **hosted**,
not local fallback or a private remote. The native macOS receipt must not report
a remote build. Full human-readable and JSON reports are retained to inspect
compiler timings and waste findings. No speedup is asserted without CI results.

Artifact transfer uses upload-artifact v4's immutable ID from the producer job
and download-artifact in the **same run**, without an external run ID, URL, or
`workflow_run` input. Consumers compare the manifest digest to a producer job
output, then validate source/run/attempt, contract, architecture and binary
checksum before execution. A rerun that mixes attempts fails closed; rerun the
whole workflow instead of just a downstream failed job. This is trusted-workflow
integrity/provenance, **not independent cryptographic build attestation**. A
compromised trusted source, builder, or workflow is outside this guarantee.

## macOS and Apple SDK boundary

Today `macos-arm64` uses a native `macos-15` producer, then transfers the binary
to a separate native `macos-15` consumer using exactly the same ID/digest gates.
It is a manual fallback producer, **not a hosted macOS cross-build**. It consumes
Apple's SDK installed on that macOS runner and does not copy or upload the SDK
to Linux or hosted storage. Enabling a future hosted producer requires an
explicit reviewed producer change, a lawfully supplied SDK with appropriate
license authorization, exact SDK/toolchain provenance, and equivalent same-run
verification. There is intentionally no arbitrary external Mach-O import input.

### Explicit optional signing and notarization

Before enabling signing, create GitHub environment `native-handoff-signing`,
restrict deployment branches to `master`, and configure required reviewers.
Store these secrets **in that environment**, not in the build job:

- `APPLE_CERTIFICATE_P12_BASE64`: Developer ID Application certificate plus key
- `APPLE_CERTIFICATE_PASSWORD`: password for that P12
- `APPLE_SIGNING_IDENTITY`: exact Developer ID Application signing identity
- `APPLE_NOTARY_KEY_P8_BASE64`: authorized App Store Connect notarization API key
- `APPLE_NOTARY_KEY_ID` and `APPLE_NOTARY_ISSUER`: that key's identifiers

After authorization, a maintainer can request:

```bash
gh workflow run native-handoff.yml --repo 1jehuang/jcode --ref master \
  -f lane=macos-arm64 -f sign_macos=true
```

This opt-in is not part of the initial no-secrets run. Signing only follows a
successful native verification of the trusted same-run artifact, with another
provenance check before credentials enter a step. Missing credentials fail,
never fall back to ad-hoc/unsigned output. A temporary keychain is deleted on
exit, signing uses hardened runtime and a timestamp, `codesign --verify` must
pass, and notarization must return `Accepted`. The signed ZIP, notarization
result, and separate signed/unsigned digest record are Actions artifacts only.
A raw CLI cannot carry a stapled ticket: this does not claim offline Gatekeeper
acceptance, app-bundle validation, or publication. Azure Windows credentials
and `id-token: write` are not used here.

## Local validation and readiness

```bash
python3 -m unittest discover -s scripts -p test_native_handoff.py
actionlint .github/workflows/native-handoff.yml
bash -n scripts/sign_native_handoff_macos.sh
shellcheck scripts/sign_native_handoff_macos.sh
```

Tests exercise both binary formats, wrong architecture, tampering, changed
source/run/attempt/lock/contract, symlink rejection, execution receipts, isolated
smoke invocations/timeouts, and missing signing credentials. These are fixture
and static checks, not native acceptance. CI must still demonstrate hosted
compiler availability, full Jcode cross-linking and native launches before this
can be called runtime-validated. macOS cross-SDK provisioning and live Apple
signing remain pending separately authorized inputs.
