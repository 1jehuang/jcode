# Embedded Skills Harness Status

This checklist tracks the fork proposal described in `docs/SKILLS_HARNESS.md` and `docs/CODEX_BOOTSTRAP.md`.

## Proposal pillars

| Pillar | Status | Evidence | Remaining work |
| --- | --- | --- | --- |
| Offline built-in skills | Done | `src/skill_pack.rs` embeds `karpathy-guidelines`, `optimization`, and `clean-code-guardian` with `include_str!`; unit tests assert all built-ins parse; e2e coverage asserts `skills doctor` duplicate reporting. | Keep duplicate reporting deterministic as new origins are added. |
| Deterministic skill source priority | Done | `src/skill.rs` loads built-ins, `.claude/skills`, `~/.jcode/skills`, then project `.jcode/skills`; unit tests cover built-in, Claude compat, and project-local override precedence; e2e coverage verifies isolated global precedence via `JCODE_HOME`. | Keep precedence docs and tests in lockstep with any new source. |
| Skills CLI | Done | `jcode skills list/show/sync/doctor` and `jcode-harness skills ...` are wired through `src/cli/commands.rs` and `src/bin/harness.rs`; e2e tests cover list, show, sync, doctor, duplicate reporting, and broken-pipe consumers exit cleanly. | Add JSON output modes if automation needs structured skill reports. |
| Clean Code quality gate | Done | `src/clean_code.rs`, `.jcode/quality/clean-code-rules.yaml`, and `clean-code check/rules`; e2e tests cover JSON and fail-on behavior. | Expand rule-specific fixtures as the rule pack grows. |
| `jcode-harness run` | Done | `src/bin/harness.rs` delegates to provider init, `Registry::new`, and `Agent` runtime, with JSON/NDJSON/dry-run modes; e2e dry-run tests cover skill preface selection. | Add JSON/NDJSON provider-backed smoke when a mock provider path is available. |
| Deterministic skill router | Done | `src/skill_router.rs` supports `auto`, `off`, `always`, explicit skills, coding terms, and perf terms, with unit and CLI dry-run coverage for proposal guarantees. | Keep trigger vocabulary conservative and test every expansion. |
| Harness smoke | Done | `jcode-harness smoke` executes deterministic tool cases without model calls. | Add CI-friendly smoke assertion or e2e wrapper. |
| Runtime offline assumption | Done | Runtime skill loading uses embedded strings and local paths only. | Add a test preventing accidental network/process dependency in built-in skill loading. |
| Documentation and discoverability | Partial | README, `docs/SKILLS_HARNESS.md`, `docs/CODEX_BOOTSTRAP.md`, and `.jcode/SKILLS_PLAN.md`. | Keep this status checklist updated after each implementation slice. |

## Latest validation snapshot

Commands recently run successfully:

- `cargo test -p jcode-tui-style`
- `cargo check -p jcode`
- `selfdev build` for the TUI binary
- `cargo run -q -p jcode --bin jcode -- skills list`
- `cargo run -q -p jcode --bin jcode-harness -- skills list`
- `cargo run -q -p jcode --bin jcode-harness -- smoke`
- `cargo test -p jcode skill_router --lib`
- `cargo run -q -p jcode --bin jcode-harness -- skills doctor \| head -5`
- `cargo run -q -p jcode --bin jcode -- skills list \| head -3`
- `cargo test -p jcode skill::tests --lib`
- `cargo test --test e2e harness_cli`
- `cargo test --test e2e harness_cli -- --nocapture`

## Next implementation slices

1. Add JSON/NDJSON `jcode-harness run` smoke once a mock provider path is available.
2. Add CLI regression tests for broken-pipe consumers once a binary test harness can assert pipe behavior portably.
3. Consider structured `skills list/show/doctor --json` output for external automation.
