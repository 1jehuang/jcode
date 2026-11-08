# Specifications — required behaviour and acceptance criteria

## A. PR #187 — fork delivery and release path

### A1. Binary naming

- The root package builds a binary named **`kcode`**.
- Workflows previously referenced **`jcode`**. All delivery/release references must resolve to the
  actual `kcode` artifact name.
- **Acceptance:** Windows cross-target job and release automation both locate the produced binary.

### A2. Fork release gate (`release.yml`)

Release publishing must admit **either**:

```yaml
# upstream
github.repository == '1jehuang/jcode'
# fork
github.repository == 'KooshaPari/KCode' && contains(github.ref_name, '-k')
```

- **Pre-build validator:** a `fork_version::parse` step must run **before** the build so a malformed
  fork tag fails loudly and early, rather than failing after a long build.
- **Homebrew tap publishing stays pinned to upstream only.** The fork must not publish a tap.
- **Acceptance:** an upstream release still works unchanged; a fork release with a `-k` tag is
  admitted; a malformed fork tag fails at the validator step.

### A3. Embedding cohort timeout

- Timeout budget: **1800s** (was 600s).
- Rationale: budget covers a cold compile, since `jcode-embedding` is not pre-built (no
  `default-members`).
- **Acceptance:** embedding cohort passes; measured test time 26s once compiled.

### A4. ARM64 gating — complete coverage

Gating must apply to **all four** publish paths:

| # | Path | Requirement |
|---|---|---|
| 1 | direct artifact upload | skip when ARM64 unavailable |
| 2 | blanket `find`-based upload | skip when ARM64 unavailable |
| 3 | expected-asset validation | do not demand ARM64 |
| 4 | `SHA256SUMS` | do not include ARM64 |

- **Regression requirement:** two SDK consumers that demanded the ARM64 asset **unconditionally**
  must be corrected, so gating does not break them.
- **Acceptance:** a run without ARM64 artifacts publishes successfully end to end.

### A5. Test-size ratchet

- Rebaselined onto current `master`.
- **Every changed entry must be EXACT** (`budget == measured line count`) so no slack is introduced.
- `session_tests/cases.rs`: **2959 -> 2913** (tightened, i.e. smaller).
- **Acceptance:** ratchet passes with zero slack on changed entries.

### A6. Formatting

- `cargo fmt` applied to 2 files `master` left unformatted:
  - `crates/jcode-base/src/scratch_maintenance.rs`
  - `crates/jcode-base/src/test_temp.rs`
- **Acceptance:** Format check passes.

### A7. Fork PR runner isolation (`ci.yml`)

Fork PRs must **not** run on the self-hosted desk runners.

```yaml
if: github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository
permissions:
  contents: read
```

- **Acceptance:** a PR from a fork cannot schedule onto desk self-hosted runners (closes host-RCE
  path).

### A8. gpt-5.6-sol flake

- `test_openai_supports_codex_models` must populate the scope its credential **actually reads**:
  `populate_account_models_for_scope(&provider.catalog_scope(), ...)`.
- **Acceptance:** full suite passes **both orderings**; 25/25 runs at `--test-threads=16`.

### A9. Review hygiene

- 45 review threads triaged and resolved.
- 16/16 CI checks pass.

## B. PR #256 — untrusted interpolation removal

- **Requirement:** no `${{ }}` expression bearing untrusted context may appear inside a `run:` block.
- Targets in `publish-github-packages.yml`:
  - **8** × `inputs.jcode_release_tag` -> `env: RELEASE_TAG`
  - **3** × `github.actor` -> `env: GHCR_USER`
- **Removal:** a redundant `env GHCR_TOKEN=... crane auth login` command that exposed the token in
  `/proc/<pid>/cmdline` (SonarCloud-flagged; touching the line made the pre-existing defect count
  against new code).
- **Definition of done:** repo-wide scan for `inputs.*`, `github.event.*`, `github.head_ref`,
  `github.actor`, `github.triggering_actor` inside `run:` blocks returns **0**.
- **Correctness requirement:** every step dereferencing `$VAR` must define `VAR` in its **own**
  `env:`; no duplicate YAML mapping keys.

## C. Desk host CI stability

### C1. `export-wsl.ps1` safety

1. **Refuse to run** while any `Runner.Worker` is active in the distro (override with `-Force`).
2. Export to `<OutTar>.partial`; promote over the previous backup **only on success**.
3. **Restart** the discovered runner services afterwards.

- **Acceptance:** dry-run prints `ABORT: 5 GitHub Actions runner worker(s) active` and **exits 2**
  (exit-code propagation confirmed). Original preserved as `export-wsl.ps1.bak-20261107`.

### C2. Runner reinstallation

- Source: **v2.338.2 linux-x64** package.
- Configure:

```
config.sh --unattended --url https://github.com/KooshaPari/KCode --token <t> \
  --name desk-kcode-N --labels self-hosted,desk-linux --work _work --replace
```

- Provide `runsvc.sh` wrapper (systemd units run `ExecStart=<runner-dir>/runsvc.sh`).
- **Acceptance:** both runners online with labels `self-hosted,Linux,X64,desk-linux`.

### C3. `/dev/null` mode

- Persistent systemd unit `devnull-mode.service` sets `/dev/null` mode **666**; must survive reboot
  (verified across a full reboot).
