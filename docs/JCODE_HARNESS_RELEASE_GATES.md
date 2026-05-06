# jcode-harness Release Readiness Gates

This document defines the minimum gates for calling this fork a releasable standalone `jcode-harness` product. A release is not ready just because the code compiles. It must satisfy the CLI contract, offline behavior, deterministic skills, quality gates, documentation, and upstream-divergence review below.

## Gate 1: Fork identity and scope

**Goal:** The release must clearly behave as `jcode-harness`, not as an undocumented upstream jcode variant.

**Required evidence:**

- `docs/JCODE_HARNESS_PRODUCT_PLAN.md` describes product thesis, principles, and next milestones.
- `docs/SKILLS_HARNESS.md` documents public harness commands and examples.
- `docs/JCODE_HARNESS_JSON_SCHEMAS.md` documents stable automation-facing JSON contracts.
- `docs/JCODE_HARNESS_INIT_SWARM.md` documents `/init` multi-agent bootstrap behavior.
- `docs/SKILLS_HARNESS_STATUS.md` lists implemented pillars, remaining work, and validation snapshot.

**Checks:**

```bash
test -s docs/JCODE_HARNESS_PRODUCT_PLAN.md
test -s docs/SKILLS_HARNESS.md
test -s docs/JCODE_HARNESS_JSON_SCHEMAS.md
test -s docs/JCODE_HARNESS_INIT_SWARM.md
test -s docs/SKILLS_HARNESS_STATUS.md
```

## Gate 1b: Interactive `/init` swarm bootstrap

**Goal:** `/init` must perform real LLM-driven project analysis by default, not only static file generation.

**Acceptance criteria:**

- Static scaffold files are written first, including `.jcode/init/SWARM_ANALYSIS_PLAN.md` and `.jcode/init/SWARM_ANALYSIS_REPORT.md`.
- Default `/init` queues a hidden LLM turn that instructs the model to use parallel swarm agents.
- Required discovery roles include architecture, QA, documentation/onboarding, and tooling/security.
- Synthesis is explicitly blocked on an await/report barrier before final files are written.
- `/init --no-swarm` remains available for deterministic scaffold-only recovery and tests.

**Checks:**

```bash
cargo test -p jcode project_init --lib -- --nocapture
cargo test -p jcode test_init_command --lib -- --nocapture
```

## Gate 2: Deterministic embedded skills

**Goal:** Built-in harness skills must be available offline and loaded with deterministic precedence.

**Acceptance criteria:**

- Built-ins include `karpathy-guidelines`, `optimization`, and `clean-code-guardian`.
- Source precedence remains: built-in < `.claude/skills` < `~/.jcode/skills` < project `.jcode/skills`.
- Duplicate skill names are discoverable via `skills doctor`.
- JSON output for skills commands remains machine-readable.

**Checks:**

```bash
cargo test -p jcode skill::tests --lib
cargo test --test e2e harness_cli -- --nocapture
cargo run -q -p jcode --bin jcode-harness -- skills list --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills doctor --json | python3 -m json.tool >/dev/null
```

## Gate 3: Harness run contract

**Goal:** `jcode-harness run` must be scriptable and testable without network credentials.

**Acceptance criteria:**

- `--dry-run` prints the final skill-prefaced prompt without provider calls.
- `--json` emits one JSON report with `session_id`, `provider`, `model`, `text`, and `usage`.
- `--ndjson` emits `start` and `done` events.
- `--mock-response` exercises the real Agent runtime offline.

**Checks:**

```bash
cargo test --test e2e harness_cli -- --nocapture
cargo run -q -p jcode --bin jcode-harness -- run "review this diff" --json --mock-response ok | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- run "review this diff" --ndjson --mock-response ok | while read -r line; do printf '%s\n' "$line" | python3 -m json.tool >/dev/null; done
```

## Gate 4: Clean Code Guardian quality gate

**Goal:** The offline quality gate must detect supported high-signal issues without requiring an LLM.

**Acceptance criteria:**

- Built-in rule pack parses.
- CLI `clean-code check --json` reports deterministic findings.
- `--fail-on` exits non-zero at or above the requested threshold.
- Rule-specific fixtures cover function length, file length, long lines, silent error swallowing, allow comments, path recursion, skip dirs, and clean files.

**Checks:**

```bash
cargo test -p jcode clean_code --lib
cargo test --test e2e harness_cli -- --nocapture
cargo run -q -p jcode --bin jcode-harness -- clean-code rules >/tmp/jcode-clean-code-rules.yaml
```

## Gate 5: Build and formatting

**Goal:** The release candidate must be formatted and compile in the expected development profile.

**Checks:**

```bash
cargo fmt --check
cargo check -p jcode
scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode
```

When using the Jcode self-development harness, prefer:

```text
selfdev build target=auto
```

## Gate 6: Documentation and schema compatibility

**Goal:** Automation-facing behavior must be documented before it is treated as stable.

**Acceptance criteria:**

- New JSON fields are additive and backward-compatible.
- Breaking CLI behavior changes require an explicit migration note.
- Examples in docs are runnable or intentionally marked as conceptual.

**Checks:**

```bash
cargo run -q -p jcode --bin jcode-harness -- skills list --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills show karpathy-guidelines --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills doctor --json | python3 -m json.tool >/dev/null
```

## Gate 7: Upstream divergence review

**Goal:** Intentional fork behavior must be distinguishable from accidental drift.

**Acceptance criteria:**

- Harness-specific behavior is named in docs as `jcode-harness` behavior.
- Upstream-compatible reuse remains behind existing `jcode` paths where practical.
- Divergence is captured in commits and release notes.

**Suggested release note sections:**

- Harness CLI changes
- Embedded skills and routing changes
- Quality gate changes
- Provider/runtime compatibility
- Known gaps and opt-in integration tests

## Release decision

A release candidate can be marked ready only when all mandatory gates pass and `docs/SKILLS_HARNESS_STATUS.md` has an updated validation snapshot. If a gate is intentionally skipped, the release notes must list the reason, risk, and follow-up owner.
