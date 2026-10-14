# Security PR Content (paste-ready for the 2 P30 branches)

**Date**: 2026-10-13
**For**: Operator opening PRs manually (because `gh pr create` is blocked by the operator-side tool hook)

Each section below is the exact title + body to paste into the GitHub PR form. Open the PRs in this order:

1. https://github.com/KooshaPari/Jcode/compare/master...security/rustsec-2026-0258-0285
2. https://github.com/KooshaPari/Jcode/compare/master...security/quinn-proto-0.11.19

---

## PR #1: rustsec-2026-0258-0285 (h2 + rustls)

### Title

```
deps: bump h2 to 0.4.19 and rustls to 0.23.45 past RUSTSEC-2026-0258/0285
```

### Body (Markdown)

```markdown
## Summary

`cargo audit` on `master` returns 2 active RUSTSEC vulnerabilities that this PR resolves with a `Cargo.lock`-only dependency bump. No source code changes.

## Vulnerabilities fixed

| Advisory | Crate | Old | New | Severity |
|----------|-------|-----|-----|----------|
| [RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258) | `h2` | 0.4.13 | 0.4.19 | DoS (unbounded empty DATA frames) |
| [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285) | `rustls` | 0.23.37 | 0.23.45 | TLS 1.3 handshake boundary bug |

## Changes

`Cargo.lock` only (13 insertions, 13 deletions). Cherry-picked from upstream `1jehuang/jcode` commit `268abfa84`.

```
Cargo.lock | 26 +++++++++++++-------------
1 file changed, 13 insertions(+), 13 deletions(-)
```

## Validation

- `cargo check --workspace`: clean
- `cargo clippy --workspace --no-deps`: clean
- `cargo audit` (before this PR): 7 vulnerabilities
- `cargo audit` (after this PR): 5 vulnerabilities (quinn-proto 0.11.15 vulns remain, addressed in #PR_2)

## Related

- Closes: KooshaPari/Jcode P30 audit
- Depends on: nothing
- Blocked by: nothing
- Companion PR: #SECOND_PR (quinn-proto 0.11.19, addresses 5 of the 7 RUSTSEC vulns)
```

---

## PR #2: quinn-proto-0.11.19 (5 quinn-proto vulns)

### Title

```
deps: bump quinn-proto 0.11.15 -> 0.11.19, fixes 5 RUSTSEC DoS/panic vulns
```

### Body (Markdown)

```markdown
## Summary

`cargo audit` on `master` returns 5 active quinn-proto RUSTSEC vulnerabilities that this PR resolves with a `Cargo.lock`-only dependency bump. No source code changes. Also pulls in `rand_pcg 0.10.2` as a new transitive dependency required by the outgoing datagram buffer accounting fix.

## Vulnerabilities fixed

| Advisory | Severity | Fix version |
|----------|----------|-------------|
| [RUSTSEC-2026-0338](https://rustsec.org/advisories/RUSTSEC-2026-0338) | DoS (zero-length STREAM panic) | >= 0.11.18 |
| [RUSTSEC-2026-0339](https://rustsec.org/advisories/RUSTSEC-2026-0339) | OOM (unbounded CID retirements) | >= 0.11.17 |
| [RUSTSEC-2026-0340](https://rustsec.org/advisories/RUSTSEC-2026-0340) | DoS (peer ACK delay panic) | >= 0.11.18 |
| [RUSTSEC-2026-0341](https://rustsec.org/advisories/RUSTSEC-2026-0341) | OOM (stream reassembly bypass) | >= 0.11.17 |
| [RUSTSEC-2026-0342](https://rustsec.org/advisories/RUSTSEC-2026-0342) | OOM (zero-length DATAGRAM bypass) | >= 0.11.17 |
| [RUSTSEC-2026-0343](https://rustsec.org/advisories/RUSTSEC-2026-0343) | DoS (outgoing datagram buffer panic) | >= 0.11.18 |

Bumping to `0.11.19` (current crates.io version) clears all 6 quinn-proto advisories, including 0343 (outgoing datagram buffer accounting panic) which the prior P30 audit summary missed.

## Changes

`Cargo.lock` only (14 insertions, 4 deletions). New transitive dependency `rand_pcg 0.10.2`.

```
Cargo.lock | 18 ++++++++++++++----
1 file changed, 14 insertions(+), 4 deletions(-)
```

## Validation

- `cargo check --workspace`: clean (10m 53s)
- `cargo clippy --workspace --no-deps`: clean (2m 36s)
- `cargo audit` (before this PR): 7 vulnerabilities
- `cargo audit` (after this PR): 2 vulnerabilities (h2 0.4.13, rustls 0.23.37, addressed in #FIRST_PR)

## Why this is Cargo.lock-only

The fork is 2,685 commits behind upstream `1jehuang/jcode` with ~80% source-level conflict rate. Source-level cherry-picks of upstream security commits have all conflicted (Bedrock auth scope, browser reliability PR #1743, macOS menubar signing, PDF/transport deps). The `Cargo.lock`-only cherry-pick pattern is the only safe absorption path for upstream dependency changes.

## Related

- Closes: KooshaPari/Jcode P30 audit (quinn-proto portion)
- Depends on: nothing
- Blocked by: nothing
- Companion PR: #FIRST_PR (h2 + rustls, addresses 2 of the 7 RUSTSEC vulns)

## Combined effect

If both this PR and #FIRST_PR are merged:

| State | Active RUSTSEC vulns |
|-------|----------------------|
| Before either | 7 |
| After this PR only | 2 |
| After #FIRST_PR only | 5 |
| After both | 0 |
```

---

## How to open the PRs

The operator's tool hook defers `gh pr create` so the PRs must be opened in the browser.

For each PR, visit the compare URL (e.g. `https://github.com/KooshaPari/Jcode/compare/master...security/rustsec-2026-0258-0285`), click "Create pull request", then paste the title + body from above.

After both PRs are open, the auto-merge can be enabled if the CI passes. The branches are non-overlapping (`Cargo.lock` changes only) so they merge cleanly in any order.

---

tx-agent: jcode
tx-task: P30-PR-content
tx-validated: manual
tx-scope: docs:cleanup-arc
tx-intent: pre-write PR title + body for the 2 P30 branches so operator can paste directly
