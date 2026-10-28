# Hosted preview release compilation

## Scope

The preview workflow uses hosted justrust for the two GNU/Linux release targets.
Optimization remains level 3, incremental compilation remains disabled, and
exact-source CI, quiet-period, complete-asset and publication checks remain in
place. Hosted failures must fail the build rather than silently compile locally.

The CI API key is passed only to the hosted build step. It is not embedded in
release metadata or artifacts. Native builds use the checksum-verified published
`v0.1.0-alpha.17` client. Even published `v0.1.0-alpha.19` does not yet
support the managed hosted Linux commands, despite sharing a version string
with newer source builds.
Hosted steps therefore bootstrap reviewed immutable source revision
`5c8c6e653d1874923cf94b93dd96aa3d312343f0` with Rust 1.99.0. The small
client bootstrap is local, application release compilation is hosted. A
capability preflight prevents silently using the older published client.

Linux x86-64 must retain the glibc 2.17 ABI baseline, vendored OpenSSL,
wrapper-plus-payload archive layout and baseline-container runtime checks.
Linux ARM64 must retain rebuilt standard libraries and emulated TLS, default
features, architecture validation and native ARM64 version execution before
publication. Cross-compilation alone is not native runtime acceptance.

## Executed acceptance, October 28, 2026

The exact source bootstrap completed in 42.35 seconds (task `9326036828`),
including source retrieval, client installation and the `zigbuild --help`
capability preflight. Native client installation remains unchanged.

Both release helpers were then executed using that exact bootstrapped client:

| Target | justrust run | Hosted end-to-end observation | Acceptance |
| --- | --- | --- | --- |
| Linux x86-64 | `20261028-034030000-721461` | 104.3 seconds | GLIBC maximum 2.17, vendored OpenSSL, real read-only/no-network/unprivileged manylinux execution, version and four cloud command help checks |
| Linux ARM64 | `20261028-034216041-728365` | 115.5 seconds | GLIBC maximum 2.39, AArch64 ELF, emulated TLS symbols and no native PT_TLS segment |

The preview override was `v1.0.0-preview.42`. Both artifacts retained honest
source snapshots, which differed because of a concurrent coordinator commit.
The x86 binary executed with the expected preview version. ARM64 inspection
verified embedded preview metadata, not native execution. The workflow retains
native ARM64 execution before packaging. No local application compilation
fallback was used.

The separate portable packaging script passed actual baseline-container runtime
acceptance (task `748242iynq`, 12.5 seconds) and wrapper-plus-payload archive
checks. Offline contracts additionally test compiler/transport failures,
invalid target/version/flags, corrupt or wrong-architecture artifacts, source
pin mismatch, missing client capabilities, packaging failure and credential
scoping. These timings use available hosted caches and are observations, not
cold-cache measurements or a controlled speedup claim. Live GitHub execution
remains the deployment acceptance gate.

## Remaining native targets

macOS compilation remains on native GitHub runners until a lawfully supplied
Apple SDK is provisioned on the hosted account and selected by its managed
content ID. A successful hosted Mach-O build still requires native execution
and signing verification. No SDK or signing keys are uploaded with project
source.

Windows MSVC compilation remains native until explicit Microsoft SDK consent
and the managed LLVM/SDK prerequisites are configured for the hosted account.
The workflow must not infer consent, silently substitute Windows GNU, or treat
cross-compilation as passing Windows runtime tests.

These are prerequisites for a further migration, not a claim that hosted
cross-compilation is intrinsically unsupported.

## Baseline and measurement

Successful preview workflow `40640538726` took about 74 minutes end to end.
Its source authorization job took about 21 minutes. The slowest matrix job,
Intel macOS, took about 53 minutes, including 48 minutes of compilation and
about 3 minutes saving its cache. Linux x86-64 took about 52 minutes.

Those jobs ran in parallel. Migrating Linux alone does not remove the Intel
macOS critical path, so no overall pipeline speedup is claimed until measured.
Compare actual job timestamps and hosted justrust reports after rollout,
including source sync, compilation, artifact transfer, runtime checks and
packaging. Prior Linux hosted trials establish feasibility, not a controlled
before/after result for the current source.
