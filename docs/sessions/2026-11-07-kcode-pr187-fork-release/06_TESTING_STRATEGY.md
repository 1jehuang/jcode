# Testing Strategy — how the work was validated

## Principle

Every change was validated against a **real** CI check or a **real** measured run, not an assertion
that the change "looks right". Where a check could not be produced (the flake's failing order), that
limitation is recorded rather than papered over.

## 1. PR #187 CI matrix (16/16 passing)

| Check | What it proves |
|---|---|
| Quality Guardrails | repo policy / lint gate |
| Build & Test (desk-linux) | full compile + suite on self-hosted desk runner |
| Cargo Check & Parity | workspace check + parity vs upstream |
| Format | `cargo fmt --check` |
| Clippy | lint cleanliness |
| TypeScript SDK | SDK consumer compiles (covers ARM64 consumer fix) |
| Security preflight | injection / policy scan |
| Windows Cross-Target | `kcode` binary name resolves on the Windows target |
| Release Automation | release gate + `fork_version::parse` validator |
| SonarCloud | S7630 script-injection resolved |
| semgrep | static analysis |
| Socket (x2) | dependency analysis |
| CodeRabbit | review bot |
| Kilo | review bot |
| Require Linked Issue | process gate |

**Build & Test step 17 completed in 43m08s** — this is the run that proves the gpt-5.6-sol fix in a
real CI environment.

## 2. gpt-5.6-sol flake verification

### Acceptance evidence

1. **25/25 full-suite runs** at `--test-threads=16` pass.
2. Both lock-acquisition **orderings** pass.
3. CI Build & Test passed (step 17, 43m08s).

### What was NOT achieved

No deterministic local reproduction of the **failing order** was obtained. The failure is a
scheduling artifact; the mechanism was proven by reading the code and by the exact scope fingerprint
match. This is recorded explicitly so a future reader does not assume a red/green local repro exists.

### Why `--test-threads=16` matters

Higher parallelism increases the chance of hitting the adverse interleaving. Testing at 16 threads is
stronger evidence than a serial run, which would have masked the bug entirely.

## 3. Embedding cohort

- Budget raised to 1800s.
- Measured: **26s** once the crate is compiled, confirming the budget covers compile, not test.
- **Acceptance:** cohort passes within budget.

## 4. Test-size ratchet

- Rebaselined onto current `master`.
- **Invariant under test:** every changed entry is **EXACT** (`budget == measured`). This is the
  anti-regression condition: exact budgets mean no slack was silently granted.
- `session_tests/cases.rs`: 2959 -> 2913 (tightened).

## 5. Workflow interpolation verification (PR #256)

### Static scan (primary acceptance)

Repo-wide scan for untrusted context inside `run:` blocks:

- `inputs.*`
- `github.event.*`
- `github.head_ref`
- `github.actor`
- `github.triggering_actor`

**Result: 0 matches.**

### Per-step env presence audit

For every step referencing `$RELEASE_TAG` or `$GHCR_USER`, confirm the variable is defined in that
**step's own** `env:` block. This is what caught the two self-introduced bugs:

1. duplicate `env:` mapping key -> `RELEASE_TAG` silently dropped (last key wins)
2. a step dereferencing `$RELEASE_TAG` with no `env:` definition

### Duplicate-key check

Parse the workflow and assert no mapping contains duplicate keys. Plain YAML validation does **not**
catch this; it is legal YAML.

## 6. Desk host verification

| Test | Method | Result |
|---|---|---|
| export-wsl.ps1 worker guard | dry-run variant with active workers | `ABORT: 5 GitHub Actions runner worker(s) active`, exit **2** (propagation confirmed) |
| atomic promotion | code review of `.partial` + promote-on-success | by inspection |
| runner online | runner status after reconfigure | online, labels `self-hosted,Linux,X64,desk-linux` |
| `/dev/null` mode | `devnull-mode.service` | mode 666, survived full reboot |
| runner self-recovery | services enabled | recover on WSL restart; in-flight jobs lost |

## 7. What is NOT covered

| Gap | Risk | Mitigation |
|---|---|---|
| WSL reboot trigger | jobs die unpredictably | UNRESOLVED; runner services auto-recover, jobs must be re-run |
| `-Force` path of export-wsl.ps1 | destructive window | review discipline; not exercised in tests |
| ARM64 gating when ARM64 **is** available | untested happy path in this session | covered by release automation check structure |
| Fork release with malformed tag | validator logic | `fork_version::parse` added pre-build; needs a negative test if not already present |

## 8. Regression checklist for future changes

1. `cargo fmt --check` and Clippy clean.
2. Full suite at `--test-threads=16`, ideally twice.
3. Embedding cohort within 1800s budget.
4. Test-size ratchet: no new slack on changed entries.
5. Interpolation scan returns 0.
6. Every `$VAR` in `run:` has a same-step `env:` definition.
7. No duplicate YAML mapping keys.
8. Release gate: upstream tag passes, fork `-k` tag passes, malformed fork tag fails early.
