# Dependency Security Triage

Last reviewed: 2026-05-14

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
- `RUSTSEC-2024-0320` (`yaml-rust`) was removed from the dependency graph on 2026-03-05 by trimming `syntect` features to built-in syntax/theme dumps instead of YAML loading.
- `scripts/security_preflight.sh` ignores the four current vulnerability advisories that are explicitly triaged above (`lettre` and `rustls-webpki`) so CI can remain actionable. New vulnerabilities still fail CI by default.
- Before changing dependency versions, run:
  - `cargo check`
  - `cargo test -j 1`
  - `scripts/security_preflight.sh`
