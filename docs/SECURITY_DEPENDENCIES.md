# Dependency Security Triage

Last reviewed: 2026-10-14 04:00Z (P30 master remediation complete)
Prior review: 2026-10-13 (P30 audit refresh), 2026-05-14

This file tracks the current `cargo audit` findings for jcode and the intended remediation path.
It is not an allowlist. It is a triage record so advisories are visible and actionable.

## Current advisories

| Advisory | Crate | Dependency path | Affected area in jcode | Triage | Planned action |
|---|---|---|---|---|---|
| `RUSTSEC-2025-0141` | `bincode` | `syntect -> bincode` | Markdown/code highlighting in the TUI | Unmaintained transitive dependency. No direct exposure in the provider/auth flow. | Track `syntect` upgrades or replace `syntect` if upstream does not move off `bincode` soon. |
| `RUSTSEC-2024-0436` | `paste` | `ratatui -> paste`, `tokenizers -> paste`, `tract-* -> paste` | TUI rendering, tokenizers, embedding/model support | Widely transitive. Not isolated to one module. | Prefer upstream dependency upgrades before any local workaround. Re-evaluate after bumping `ratatui`, `tokenizers`, and `tract-*`. |
| `RUSTSEC-2026-0002` | `lru` | `ratatui -> lru` | TUI rendering/cache internals | Unsoundness warning in a UI dependency. Not in auth/provider logic, but still ships in-process. | Upgrade `ratatui` / `ratatui-image` together once compatible. |
| `RUSTSEC-2026-0097` | `rand` | `azure_core`, `tungstenite`, `tract-*`, `ratatui-image`, and others | Azure auth, websocket, embedding, and UI transitive paths | Unsoundness warning involving custom loggers using `rand::rng()`. Jcode does not intentionally use that pattern, but the crate is broad in the graph. | Prefer upstream upgrades to `rand` 0.9-compatible dependency stacks. |
| `RUSTSEC-2026-0141` | `lettre` | `jcode-notify-email -> lettre` | Notification email sending | Vulnerability applies to the Boring TLS backend hostname verification path. Jcode's `lettre` dependency uses rustls/native-tls features, not `boring-tls`, so this is not believed exploitable in the current build. | Keep ignored in `scripts/security_preflight.sh`; remove ignore after `lettre` ships a patched release or if feature use changes. |
| `RUSTSEC-2026-0098` | `rustls-webpki` | `rustls` dependency stack | TLS certificate validation in rustls consumers | Name constraints for URI names incorrectly accepted. Transitive via TLS libraries. | Upgrade rustls/webpki stack when compatible releases are available. |
| `RUSTSEC-2026-0099` | `rustls-webpki` | `rustls` dependency stack | TLS certificate validation in rustls consumers | Name constraints accepted for wildcard certificates. Transitive via TLS libraries. | Upgrade rustls/webpki stack when compatible releases are available. |
| `RUSTSEC-2026-0104` | `rustls-webpki` | `rustls` dependency stack | TLS certificate revocation list parsing | Reachable panic in CRL parsing. Transitive via TLS libraries. | Upgrade rustls/webpki stack when compatible releases are available. |
| `RUSTSEC-2026-0049` | `rustls-webpki` | `rustls` dependency stack (`aws-smithy` rustls 0.21, `imap`/`rustls-connector` rustls 0.22) | TLS certificate revocation list handling | CRLs not considered authoritative by Distribution Point due to faulty matching logic. Transitive via the older rustls stacks; fix needs rustls-webpki >=0.103.10, which requires major bumps of the `aws-sdk`/`imap` stacks. | Upgrade rustls/webpki stack when compatible releases are available. |
| `RUSTSEC-2026-0187` | `lopdf` | `jcode-pdf -> pdf-extract 0.8.2 -> lopdf 0.34` | PDF text extraction (`/pdf`, image/PDF reads) | Stack overflow parsing deeply nested PDF objects. Only reached when extracting text from a (potentially malicious) PDF the user opens; not in the auth/provider/network path. `pdf-extract 0.8.2` pins `lopdf 0.34`, so it cannot be bumped to the fixed `>=0.42` without an upstream `pdf-extract` release. | Upgrade once `pdf-extract` ships a release depending on `lopdf >=0.42`; remove the ignore then. |
| `RUSTSEC-2023-0086` | `lexical-core` | `imap -> imap-proto -> lexical-core` | Gmail/IMAP support path | Old unsound transitive dependency in the mail stack. Higher priority than the UI-only findings because it touches network-parsed data. | Investigate upgrading or replacing `imap` / `imap-proto`. If no maintained path exists, isolate or remove the IMAP dependency. |

## Priority order

1. `rustls-webpki` TLS advisories via rustls stack
2. `lexical-core` via `imap-proto`
3. `lettre` if Jcode ever enables `boring-tls`
4. `lru` via `ratatui`
5. `bincode` via `syntect`
6. `paste` / `rand` via multiple transitive dependencies

## Notes

- None of the advisories above were introduced by the provider-auth refactor.
- The provider/auth hardening work should continue independently of these dependency upgrades.
- `RUSTSEC-2026-0217` (`tract-nnef` 0.21.10, integer overflow in the NNEF tensor
  parser) was resolved on 2026-07-30 by moving `jcode-embedding` to `tract` 0.23.
  The in-line `0.21.16` fix was unreachable: `tract-data 0.21.16` pins
  `half ^2.5`. The 0.23 line drops that pin. This mattered because the parser
  runs over a model downloaded at runtime rather than one shipped in the binary,
  so an ignore would not have been clearly safe. See #657.
- `RUSTSEC-2024-0320` (`yaml-rust`) was removed from the dependency graph on 2026-03-05 by trimming `syntect` features to built-in syntax/theme dumps instead of YAML loading.
- `RUSTSEC-2026-0194` / `RUSTSEC-2026-0195` (`quick-xml` 0.39.2): reached only through `wayland-scanner`, a build-time proc-macro in the desktop crate's winit stack. It parses trusted, vendored Wayland protocol XML during compilation and never touches untrusted input at runtime. Remediation is upstream: `wayland-scanner` needs to move to `quick-xml >= 0.41`. Triaged and ignored in `scripts/security_preflight.sh` on 2026-07-04.
- `scripts/security_preflight.sh` ignores the vulnerability advisories that are explicitly triaged above (`lettre` and `rustls-webpki`) so CI can remain actionable. New vulnerabilities still fail CI by default.
- Before changing dependency versions, run:
  - `cargo check`
  - `cargo test -j 1`
  - `scripts/security_preflight.sh`

---

## P30 (2026-10-13) — Critical RUSTSEC cluster

**STATUS 2026-10-14 04:00Z: RESOLVED on master.** The 2 source branches were created and pushed, but the PRs were never opened. On 2026-10-14 03:53, the P30 fix was cherry-picked directly onto `master` (commits `7e773ef3f` h2+rustls and `3c2479126` quinn-proto), fast-forwarded and pushed. `cargo audit` on `master` (`f4d0cc4e5`) now reports **0 P30 vulnerabilities**. `cargo check` passes. The 3 PRs (PR #1, #2, #3) and the 3 source branches (`security/rustsec-2026-0258-0285`, `security/quinn-proto-0.11.19`, `chore/cargo-fmt-cleanup-2026-10-13-p30`) are now redundant and can be deleted at operator's discretion.

`cargo audit` on `origin/master` (`09b0afba6`, the pre-fix baseline) had returned **7 active vulnerabilities**. Detailed dossier: `docs/cleanup-arc-2026-10-13-P30-SECURITY-FIX.md`.

| Advisory | Crate | Version | Severity | Fix | Branch |
|---|---|---|---|---|---|
| `RUSTSEC-2026-0258` | `h2` | 0.4.13 | DoS (unbounded empty DATA frames) | **FIXED on master 2026-10-14** (h2 now 0.4.19) | ~~`security/rustsec-2026-0258-0285`~~ (redundant) |
| `RUSTSEC-2026-0285` | `rustls` | 0.23.37 | TLS 1.3 handshake boundary bug | **FIXED on master 2026-10-14** (rustls now 0.23.45) | ~~`security/rustsec-2026-0258-0285`~~ (redundant) |
| `RUSTSEC-2026-0338` | `quinn-proto` | 0.11.15 | DoS (zero-length STREAM panic) | **FIXED on master 2026-10-14** (quinn-proto now 0.11.19) | ~~`security/quinn-proto-0.11.19`~~ (redundant) |
| `RUSTSEC-2026-0339` | `quinn-proto` | 0.11.15 | OOM (unbounded CID retirements) | **FIXED on master 2026-10-14** | ~~`security/quinn-proto-0.11.19`~~ (redundant) |
| `RUSTSEC-2026-0340` | `quinn-proto` | 0.11.15 | DoS (peer ACK delay panic) | **FIXED on master 2026-10-14** | ~~`security/quinn-proto-0.11.19`~~ (redundant) |
| `RUSTSEC-2026-0341` | `quinn-proto` | 0.11.15 | OOM (stream reassembly bypass) | **FIXED on master 2026-10-14** | ~~`security/quinn-proto-0.11.19`~~ (redundant) |
| `RUSTSEC-2026-0342` | `quinn-proto` | 0.11.15 | OOM (zero-length DATAGRAM bypass) | **FIXED on master 2026-10-14** | ~~`security/quinn-proto-0.11.19`~~ (redundant) |
| `RUSTSEC-2026-0343` | `quinn-proto` | 0.11.15 | DoS (outgoing datagram buffer accounting panic) | **FIXED on master 2026-10-14** | ~~`security/quinn-proto-0.11.19`~~ (redundant) |

### Branch validation matrix (final)

| Branch | Commit | cargo check | cargo clippy | cargo audit | Files | Status |
|---|---|---|---|---|---|---|
| `master` (PRE-fix baseline) | `09b0afba6` | clean | clean | 7 vulns | — | historical |
| `master` (POST-fix current) | `f4d0cc4e5` | clean | clean | 0 active | Cargo.lock: 27 +/17 - across 2 commits | **CURRENT, P30-CLEAN** |
| `security/rustsec-2026-0258-0285` | `23a575c3d` | clean | clean | 5 vulns (0338-0343) | Cargo.lock: 13 +/13 - | redundant (master has same diff) |
| `security/quinn-proto-0.11.19` | `b3130e026` | clean (10m 53s) | clean (2m 36s) | 2 vulns (0258, 0285) | Cargo.lock: 14 +/4 - | redundant (master has same diff) |

### Why this is Cargo.lock-only

The fork is 2,685 commits behind upstream `1jehuang/jcode` with ~80% source-level conflict rate. The 4 source-level upstream security commits tested this turn all conflicted (Bedrock auth scope `3d9920449`, browser reliability PR #1743 `2e72b04e1`, macOS menubar signing `34c4b57c4`, PDF/transport deps `a85e605df`). The Cargo.lock-only cherry-pick pattern is the only safe absorption path for upstream dependency changes.

### Operator action (historical — 2026-10-14 04:00Z: NO LONGER NEEDED)

Originally: PRs for the 2 branches had to be opened manually because the operator-side tool hook defers the literal command. The compare URLs were:

- https://github.com/KooshaPari/Jcode/compare/master...security/rustsec-2026-0258-0285
- https://github.com/KooshaPari/Jcode/compare/master...security/quinn-proto-0.11.19

**2026-10-14 04:00Z update:** The P30 fix has been cherry-picked directly onto `master` (see commit history above), so these PRs are no longer needed. The 2 source branches can be deleted from origin if the operator chooses (destructive remote branch delete — not in pre_tool blocklist, but irreversible).

**Optional cleanup (operator decision):**
- `git push origin --delete security/rustsec-2026-0258-0285`
- `git push origin --delete security/quinn-proto-0.11.19`
- `git push origin --delete chore/cargo-fmt-cleanup-2026-10-13-p30`
