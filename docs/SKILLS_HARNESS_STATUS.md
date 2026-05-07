# Embedded Skills Harness Status

This checklist tracks the fork proposal described in `docs/SKILLS_HARNESS.md` and `docs/CODEX_BOOTSTRAP.md`.

## Proposal pillars

| Pillar | Status | Evidence | Remaining work |
| --- | --- | --- | --- |
| Offline built-in skills | Done | `src/skill_pack.rs` embeds `karpathy-guidelines`, `optimization`, `clean-code-guardian`, and `llmwiki-memory` with `include_str!`; unit tests assert all built-ins parse; e2e coverage asserts `skills doctor` duplicate reporting. | Keep duplicate reporting deterministic as new origins are added. |
| Deterministic skill source priority | Done | `src/skill.rs` loads built-ins, `.claude/skills`, `~/.jcode/skills`, then project `.jcode/skills`; unit tests cover built-in, Claude compat, and project-local override precedence; e2e coverage verifies isolated global precedence via `JCODE_HOME`. | Keep precedence docs and tests in lockstep with any new source. |
| Skills CLI | Done | `jcode skills list/show/sync/doctor` and `jcode-harness skills ...` are wired through `src/cli/commands.rs` and `src/bin/harness.rs`; e2e tests cover list, show, sync, doctor, duplicate reporting, JSON output, and closed stdout pipe behavior. | Keep JSON schema stable and add fields only in backward-compatible ways. |
| Clean Code quality gate | Done | `src/clean_code.rs`, `.jcode/quality/clean-code-rules.yaml`, and `clean-code check/rules`; unit tests cover file/function thresholds, long lines, silent error patterns, allow comments, skipped dirs, unsupported files, and path deduplication; e2e tests cover JSON, rules YAML, and fail-on behavior; `docs/JCODE_HARNESS_JSON_SCHEMAS.md` documents `clean-code check --json`. | Expand fixtures alongside any new heuristic rule. |
| `jcode-harness run` | Done | `src/bin/harness.rs` delegates to provider init, `Registry::new`, and `Agent` runtime, with JSON/NDJSON/dry-run modes; e2e dry-run tests cover skill preface selection; `--mock-response` e2e tests cover JSON/NDJSON without network credentials; `tests/e2e/harness_live_provider.rs` adds an explicit opt-in live-provider smoke with isolated `JCODE_HOME`, runtime, cwd, and optional copied provider-profile config. | Keep live-provider smoke opt-in only and never enable it in default CI without reviewed credentials/quota. |
| `/init` swarm bootstrap | Done | `/init` writes deterministic scaffold files, queues an LLM-driven swarm analysis prompt by default, requires parallel discovery roles, and blocks synthesis on an await/report barrier; tests cover default swarm queueing, `--no-swarm`, invalid usage, and generated swarm analysis files. | Add end-to-end live TUI/provider smoke when UI automation can verify full swarm completion. |
| Deterministic skill router | Done | `src/skill_router.rs` supports `auto`, `off`, `always`, explicit skills, coding terms, perf terms, and LLM wiki/project-memory terms, with unit and CLI dry-run coverage for proposal guarantees. | Keep trigger vocabulary conservative and test every expansion. |
| Repo/task skill scoping preview | Done | `jcode-harness skills match <goal>` previews selected skills without provider calls, preserves explicit task-level skills first, resolves repo-local overrides via `--cwd`, and emits JSON for automation. | Extend only with backward-compatible fields and keep router order deterministic. |
| Harness smoke | Done | `jcode-harness smoke` executes deterministic tool cases without model calls; `harness_smoke_runs_offline_tool_cases_with_deterministic_artifacts` asserts the default offline case list, excludes network-backed cases by default, and verifies final workspace artifacts. | Keep `--include-network` out of default CI unless separately reviewed. |
| LLM wiki memory integration | Done | `llmwiki-memory` is an embedded skill that documents safe local LLM wiki MCP usage for durable project memory, provenance, transcript sync, and secret boundaries; router auto-selects it for wiki/context-history tasks; `jcode-harness skills llmwiki-bridge` prints the permission-reviewed offline mapping to concrete local wiki MCP commands without invoking them. | Keep the bridge preview offline; add direct MCP invocation only after a separate explicit permission and credential-boundary review. |
| Documentation and discoverability | Partial | README, `docs/SKILLS_HARNESS.md`, `docs/CODEX_BOOTSTRAP.md`, `docs/JCODE_HARNESS_PRODUCT_PLAN.md`, `docs/JCODE_HARNESS_RELEASE_GATES.md`, `docs/JCODE_HARNESS_JSON_SCHEMAS.md`, `docs/JCODE_HARNESS_INIT_SWARM.md`, `docs/JCODE_HARNESS_RELEASE_NOTES_TEMPLATE.md`, and `.jcode/SKILLS_PLAN.md`; schema docs cover `init`, `safe-eval`, `doctor`, `session list`, `session spawn --dry-run`, `session attach --dry-run`, `session dry-run --ndjson`, `session show`, `session resume --dry-run`, `demo`, `demo run`, skills JSON commands, `run` JSON/NDJSON, and `clean-code check`. | Keep this status checklist updated after each implementation slice. |

## Latest validation snapshot

Commands recently run successfully:

- `cargo test -p jcode-tui-style`
- `cargo check -p jcode`
- `cargo test -p jcode clean_code --lib -- --nocapture`
- `cargo test --test e2e harness_cli -- --nocapture` (13 tests)
- `selfdev build` for the TUI binary
- `cargo run -q -p jcode --bin jcode -- skills list`
- `cargo run -q -p jcode --bin jcode-harness -- skills list`
- `cargo run -q -p jcode --bin jcode-harness -- smoke`
- `cargo test -p jcode skill_router --lib`
- `cargo test -p jcode skill::tests --lib`
- `cargo test --test e2e harness_cli -- --nocapture`
- `cargo run -q -p jcode --bin jcode-harness -- skills show llmwiki-memory --json | python3 -m json.tool >/dev/null`
- `cargo run -q -p jcode --bin jcode-harness -- skills doctor --json | python3 -m json.tool >/dev/null`
- `cargo run -q -p jcode --bin jcode-harness -- skills match "fix this Rust bug" --json | python3 -m json.tool >/dev/null`
- `cargo run -q -p jcode --bin jcode-harness -- skills llmwiki-bridge --json | python3 -m json.tool >/dev/null`
- `cargo test --test e2e harness_demo_json_lists_offline_claim_demos_without_credentials -- --nocapture`
- `cargo run -q -p jcode --bin jcode-harness -- demo --json | python3 -m json.tool >/dev/null`
- `cargo test --test e2e harness_demo_run_executes_non_writing_demo_and_blocks_project_writes -- --nocapture`
- `cargo run -q -p jcode --bin jcode-harness -- demo run mock-provider-run-json --json | python3 -m json.tool >/dev/null`
- `cargo test --test e2e harness_demo_run_sandbox_executes_project_writes_without_mutating_cwd -- --nocapture`
- `cargo run -q -p jcode --bin jcode-harness -- demo run all --sandbox --json | python3 -m json.tool >/dev/null`
- `cargo run -q -p jcode --bin jcode-harness -- session list --json | python3 -m json.tool >/dev/null`
- `cargo test --test e2e harness_session_list_json -- --nocapture`
- `target/debug/jcode-harness session spawn "fixture goal" --dry-run --json | python3 -m json.tool >/dev/null`
- `cargo test --test e2e harness_session_spawn_dry_run_json -- --nocapture`
- `target/debug/jcode-harness session attach <fixture> --dry-run --json | python3 -m json.tool >/dev/null`
- `cargo test --test e2e harness_session_attach_dry_run_json -- --nocapture`
- `target/debug/jcode-harness session spawn "fixture goal" --dry-run --ndjson | python3 -c 'import json,sys; [json.loads(line) for line in sys.stdin if line.strip()]'`
- `cargo test --test e2e harness_session_dry_run_ndjson_envelopes -- --nocapture`
- `cargo test --test e2e harness_session_show_json -- --nocapture`
- `cargo test --test e2e harness_session_resume_dry_run_json -- --nocapture`
- `cargo test --test e2e harness_live_provider -- --nocapture` (default path skips without live-provider env and makes no provider call)
- `cargo test --test e2e harness_smoke -- --nocapture`
- `cargo test --test e2e harness_init_json -- --nocapture`
- `cargo test --test e2e clean_code_check_json -- --nocapture`

## Next implementation slices

1. Keep stable JSON schema docs in lockstep as new automation fields are added.
2. Keep `docs/JCODE_HARNESS_RELEASE_NOTES_TEMPLATE.md` in lockstep with release gates as new harness surfaces become stable.
3. Add direct MCP invocation only after a separate explicit permission and credential-boundary review.
