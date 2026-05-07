# jcode-harness

> A harness-first fork of [jcode](https://github.com/1jehuang/jcode) that combines fast multi-session agent workflows with offline embedded skills, local LLM wiki memory, and deterministic quality gates.

[![License](https://img.shields.io/github/license/1jehuang/jcode?style=flat-square)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-blue?style=flat-square)](https://github.com/1jehuang/jcode/releases)

![jcode-harness engineering loop](docs/assets/jcode-harness-loop.svg)

## At a glance

| Layer | What it contributes | Durable artifact |
| --- | --- | --- |
| Jcode runtime | Fast Rust CLI/TUI, tools, providers, sessions, swarm, side panel, self-dev builds | `src/`, `crates/`, logs, session state |
| Embedded skills | Offline behavioral instructions and deterministic source precedence | `src/skill_pack.rs`, `.jcode/skills/*/SKILL.md` |
| Karpathy guidelines | Practical agent discipline: plan, keep changes surgical, avoid overengineering, verify success | `karpathy-guidelines` built-in skill, vendored source in `third_party/` |
| LLM wiki memory | Prior decisions, transcripts, provenance, handoff context, searchable project memory | wiki pages/raw sessions via local MCP tools |
| Harness governance | `/init` plans, release gates, clean-code checks, JSON/NDJSON contracts, validation snapshots | `.jcode/`, `docs/JCODE_HARNESS_*`, e2e tests |

## What this fork is

This branch is not only a small patch set on top of upstream jcode. It is a product direction called **jcode-harness**.

The goal is to turn jcode into a rigorous local AI engineering harness:

- **Jcode core** supplies the fast Rust CLI/TUI, provider integration, tools, sessions, swarm coordination, self-development flow, side panel, memory, and automation surface.
- **LLM wiki** supplies durable project memory: prior decisions, session transcripts, provenance, handoff context, and searchable project knowledge.
- **Karpathy-inspired skills** supply behavioral guardrails for agent work: think before coding, keep changes surgical, avoid speculative abstractions, and define verifiable success criteria.
- **Harness quality gates** supply deterministic checks before claims of completion: JSON/NDJSON contracts, offline skills, clean-code checks, init swarm analysis, and repeatable tests.

In short: this fork is about making an AI coding agent less improvisational and more like a disciplined engineering runtime.

## How the engineering is put together

The engineering is built as a closed local loop, similar in spirit to the Codex Harness MCP loop: request, context, contract, execution, evidence, gate, and handoff. The difference is that this fork embeds that loop directly into Jcode's Rust runtime and project files.

1. **Request enters Jcode** through the interactive TUI, `jcode run`, `jcode-harness run`, or `/init`. The request is not treated as enough context by itself. It is paired with cwd, provider choice, skill mode, safety policy, and project-local artifacts.
2. **Project bootstrap creates durable structure** under `.jcode/`: init reports, questions, MCP plan, skills plan, side-panel status, and swarm analysis files. This prevents the first agent turn from being a pure chat transcript with no durable output.
3. **The swarm analysis separates concerns**. Architecture, QA, documentation/onboarding, and tooling/security are discovered independently, then synthesis waits on a barrier before writing recommendations.
4. **The skill router narrows behavior**. Coding work gets `karpathy-guidelines` plus `clean-code-guardian`; performance work gets `optimization`; project-memory or prior-decision work gets `llmwiki-memory`. Explicit skills always win, and automatic routing stays conservative.
5. **The LLM wiki is the memory plane**. It is used for prior decisions, transcripts, provenance, and handoff context. It is deliberately not treated as source-code truth, so code claims still need repository/test evidence.
6. **Verification gates close the loop**. `cargo fmt`, focused tests, e2e harness tests, JSON schema checks, clean-code checks, and self-dev builds are the evidence that a change is real.
7. **Artifacts make the work resumable**. Future agents can read README, `docs/CODEX_BOOTSTRAP.md`, `.jcode/init/SWARM_ANALYSIS_REPORT.md`, `.jcode/SKILLS_PLAN.md`, and side-panel status instead of reconstructing intent from chat history.

The image above captures that loop: Jcode receives work, `/init` and swarm analysis create structure, skills constrain behavior, agent runtime performs the task, LLM wiki memory preserves decisions, verification gates prove completion, and durable artifacts make the next session safer.

## Why this exists

Many AI coding tools are powerful but too ephemeral:

1. They forget why earlier decisions were made.
2. They rely on prompt habits that are not enforced or tested.
3. They make broad changes without a local governance loop.
4. They require provider/network access even for behavior that could be local.
5. Their automation output is hard to trust in CI or scripts.

`jcode-harness` attacks those problems with a local-first design:

- reusable skills are embedded into the binary;
- durable knowledge is routed through the local LLM wiki;
- project bootstrap creates explicit plans, questions, risks, and status pages;
- agent runs can be scriptable and machine-readable;
- quality gates are testable without live model credentials.

## Current built-in skills

Built-in skills are compiled into the binary with `include_str!`. They do not require internet access, Node, Claude Code, Cursor, Codex CLI, or plugin marketplaces at runtime.

| Skill | Purpose |
| --- | --- |
| `karpathy-guidelines` | Behavioral guidelines adapted from [`forrestchang/andrej-karpathy-skills`](https://github.com/forrestchang/andrej-karpathy-skills). Use for disciplined coding, review, refactoring, and debugging. |
| `optimization` | Performance, memory, latency, throughput, CPU/RAM, and compile-time improvement work. |
| `clean-code-guardian` | Offline quality policy and rule pack for readable, focused, well-tested code without silent errors. |
| `llmwiki-memory` | Safe use of the local LLM wiki as durable project memory with provenance, transcript sync, prior-decision lookup, and secret boundaries. |

Skill source priority is deterministic:

1. built-in skills;
2. project compatibility skills from `./.claude/skills`;
3. global jcode skills from `~/.jcode/skills`;
4. project-local jcode skills from `./.jcode/skills`.

Later sources override earlier sources with the same skill name. This lets a project override a built-in skill without rebuilding the binary.

## How skill routing works

`jcode-harness run` can prepend selected skill context before an agent run.

The router is intentionally conservative:

- coding, bug, test, refactor, review, implement, fix, pull request, or diff tasks select `karpathy-guidelines` and `clean-code-guardian`;
- performance, latency, memory, throughput, CPU, RAM, or efficiency tasks select `optimization`;
- LLM wiki, project memory, prior decision, provenance, transcript, or context-history tasks select `llmwiki-memory`;
- explicit `--skill <name>` always includes that skill;
- `--skills off` disables automatic routing while preserving explicit skills;
- `--skills always` includes all built-in harness skills.

The router does not inject every skill by default. The point is to keep context relevant and auditable.

## LLM wiki role

The LLM wiki is the memory layer, not source-code truth.

Use it to answer questions like:

- What did we decide last time?
- Which risks were already identified?
- Which validation commands were trusted?
- Where did this architectural constraint come from?
- What should a future agent know before continuing?

But always verify code claims against the repository. Wiki memory can be stale. Source files, tests, and explicit user instructions win.

Secret policy is strict: do not sync tokens, API keys, private keys, `.env` values, provider credentials, deployment secrets, database credentials, cookies, or local session secrets into wiki memory.

## Main commands

### Interactive jcode

```bash
jcode
```

### Harness CLI

```bash
jcode-harness
jcode-harness smoke
jcode-harness safe-eval
jcode-harness safe-eval --json
jcode-harness doctor
jcode-harness doctor --json
jcode-harness init --yes
```

### Safe first run

For a cautious first evaluation, create an isolated profile before importing
credentials or enabling high-impact integrations:

```bash
jcode-harness safe-eval
source .jcode/safe-eval/safe-eval.env
jcode-harness run "say hello" --json --mock-response "safe eval ok"
```

The generated profile uses an isolated `JCODE_HOME`, disables telemetry,
ambient/proactive work, swarm auto-coordination, persistent semantic memory,
autoreview, autojudge, gateway exposure, and external credential auto-trust. It
also writes `.jcode/safe-eval/README.md` with a trust checklist and PowerShell
activation file.

Use `jcode-harness doctor --json` for offline diagnostics before running live
providers. It reports safe-eval activation, telemetry opt-out state, platform,
skill loading health, and project/global MCP config paths without contacting
model providers or starting MCP/browser/Gmail integrations.

### Skills

```bash
jcode-harness skills list
jcode-harness skills list --json
jcode-harness skills show karpathy-guidelines
jcode-harness skills show llmwiki-memory --json
jcode-harness skills sync
jcode-harness skills doctor --json
```

### Scriptable runs

```bash
jcode-harness run "review this diff" --skill karpathy-guidelines --max-turns 3 --json
jcode-harness run "query prior architecture decisions" --dry-run
jcode-harness run "optimize this Rust hot path" --skills auto --dry-run
```

For CI and contract tests, use the deterministic mock provider:

```bash
jcode-harness run "review this diff" --json --mock-response "deterministic response"
jcode-harness run "review this diff" --ndjson --mock-response "deterministic response"
```

### Clean Code Guardian

```bash
jcode-harness clean-code check --json
jcode-harness clean-code check src tests --fail-on warning
jcode-harness clean-code rules
```

## Project bootstrap with `/init`

The fork adds a harness-oriented init flow.

`/init` and `jcode-harness init` generate project-local scaffolding under `.jcode/`, including:

- `.jcode/INIT_REPORT.md`
- `.jcode/INIT_QUESTIONS.md`
- `.jcode/SKILLS_PLAN.md`
- `.jcode/MCP_PLAN.md`
- `.jcode/init/SWARM_ANALYSIS_PLAN.md`
- `.jcode/init/SWARM_ANALYSIS_REPORT.md`
- `.jcode/side_panel/status.md`

The default interactive `/init` path queues an LLM-driven swarm analysis after static scaffolding. Required discovery roles are architecture, QA, documentation/onboarding, and tooling/security. Synthesis is blocked on a report barrier before final recommendations are written.

Use deterministic scaffold-only mode when needed:

```bash
/init --no-swarm
```

## Installation

For upstream stable jcode installation:

```bash
curl -fsSL https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.sh | bash
```

For this fork or local development, build from source:

```bash
git clone https://github.com/chapzin/jcode-harness.git
cd jcode-harness
cargo build -p jcode --bin jcode
cargo build -p jcode --bin jcode-harness
```

When working inside the self-development harness, prefer coordinated builds:

```text
selfdev build target=auto
```

Fallback local build:

```bash
scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode
```

## Validation gates

Common focused checks:

```bash
cargo fmt --check
cargo check -p jcode
cargo test -p jcode project_init --lib -- --nocapture
cargo test -p jcode test_init_command --lib -- --nocapture
cargo test -p jcode skill_router --lib
cargo test -p jcode skill::tests --lib
cargo test -p jcode clean_code --lib
cargo test --test e2e harness_cli -- --nocapture
cargo run -q -p jcode --bin jcode-harness -- skills list --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills show llmwiki-memory --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills doctor --json | python3 -m json.tool >/dev/null
```

Release-readiness gates live in [`docs/JCODE_HARNESS_RELEASE_GATES.md`](docs/JCODE_HARNESS_RELEASE_GATES.md). A release candidate is not ready just because it compiles. It must satisfy CLI contracts, offline skill behavior, deterministic quality gates, documentation, JSON compatibility, and upstream-divergence review.

## Repository map

Important paths for this fork:

| Path | Meaning |
| --- | --- |
| `src/main.rs` | Primary `jcode` CLI/TUI binary. |
| `src/bin/harness.rs` | `jcode-harness` automation-facing binary. |
| `src/project_init.rs` | Init scaffolding and swarm bootstrap. |
| `src/skill.rs` | Skill loading, precedence, parsing, reload behavior. |
| `src/skill_pack.rs` | Built-in skill registry compiled with `include_str!`. |
| `src/skill_router.rs` | Deterministic task-to-skill routing. |
| `.jcode/skills/` | Project-local skill definitions, including built-in source files for this fork. |
| `.jcode/quality/` | Clean Code Guardian rule pack. |
| `third_party/andrej-karpathy-skills/` | Vendored upstream Karpathy-inspired skill material and attribution-sensitive source. |
| `docs/SKILLS_HARNESS.md` | Skills harness operating docs. |
| `docs/CODEX_BOOTSTRAP.md` | Continuation notes for future agents. |
| `docs/SKILLS_HARNESS_STATUS.md` | Implementation status and validation snapshot. |
| `docs/JCODE_HARNESS_RELEASE_GATES.md` | Release-readiness gates. |

## Security boundaries

- Built-in skill loading must remain local/offline.
- MCP setup is review-first. Do not auto-install remote MCP servers or persist credentials without explicit review.
- LLM wiki memory must never contain secrets.
- Provider/auth, telemetry, release, browser automation, and email/Gmail tooling are sensitive integration surfaces.
- Destructive or externally visible actions, such as deployment, publishing, database writes, or sending emails, require explicit confirmation.

## Compatibility with upstream jcode

This fork preserves upstream jcode behavior where practical:

- `jcode run`
- `jcode serve`
- `jcode connect`
- existing provider integrations
- the fast Rust TUI/session workflow

Fork-specific behavior is documented as `jcode-harness` behavior. The goal is not to remove upstream capabilities, but to add a disciplined harness layer around them.

## Further reading

- [Skills Harness](docs/SKILLS_HARNESS.md)
- [Clean Code Guardian](docs/CLEAN_CODE_GUARDIAN.md)
- [Product Engineering Plan](docs/JCODE_HARNESS_PRODUCT_PLAN.md)
- [Release Readiness Gates](docs/JCODE_HARNESS_RELEASE_GATES.md)
- [JSON Schemas](docs/JCODE_HARNESS_JSON_SCHEMAS.md)
- [Init Swarm Bootstrap](docs/JCODE_HARNESS_INIT_SWARM.md)
- [Codex Bootstrap](docs/CODEX_BOOTSTRAP.md)
- [Crate Ownership Boundaries](docs/CRATE_OWNERSHIP_BOUNDARIES.md)

## Attribution

This fork vendors selected Karpathy-inspired skill material from [`forrestchang/andrej-karpathy-skills`](https://github.com/forrestchang/andrej-karpathy-skills) under `third_party/andrej-karpathy-skills/` and adapts it into the built-in `karpathy-guidelines` skill. See [`NOTICE.md`](NOTICE.md).

jcode remains open source under the repository license. See [`LICENSE`](LICENSE).
