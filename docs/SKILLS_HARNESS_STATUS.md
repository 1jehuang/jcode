# Embedded Skills Harness Status

This checklist tracks the fork proposal described in `docs/SKILLS_HARNESS.md` and `docs/CODEX_BOOTSTRAP.md`.

## Proposal pillars

| Pillar | Status | Evidence | Remaining work |
| --- | --- | --- | --- |
| Offline built-in skills | Done | `src/skill_pack.rs` embeds `karpathy-guidelines`, `optimization`, and `clean-code-guardian` with `include_str!`. | Add regression tests that assert all built-ins parse and are routable. |
| Deterministic skill source priority | Done | `src/skill.rs` loads built-ins, `.claude/skills`, `~/.jcode/skills`, then project `.jcode/skills`. | Add tests for built-in override precedence and duplicate reporting. |
| Skills CLI | Done | `jcode skills list/show/sync/doctor` and `jcode-harness skills ...` are wired through `src/cli/commands.rs` and `src/bin/harness.rs`. | Improve script UX for long output and broken-pipe consumers. |
| Clean Code quality gate | Done | `src/clean_code.rs`, `.jcode/quality/clean-code-rules.yaml`, and `clean-code check/rules`. | Add focused CLI/integration tests for JSON and fail-on severity behavior. |
| `jcode-harness run` | Done | `src/bin/harness.rs` delegates to provider init, `Registry::new`, and `Agent` runtime, with JSON/NDJSON/dry-run modes. | Add dry-run regression tests for skill preface selection. |
| Deterministic skill router | Done | `src/skill_router.rs` supports `auto`, `off`, `always`, explicit skills, coding terms, and perf terms, with unit coverage for proposal guarantees. | Add CLI dry-run regression tests around the router integration. |
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

## Next implementation slices

1. Improve harness/script UX by handling long command output consumers, especially broken pipe behavior observed with `skills doctor | head`.
2. Add dry-run tests for `jcode-harness run --skills auto/off/always --skill ...`.
3. Add Clean Code CLI regression coverage for JSON and `--fail-on` thresholds.
4. Add built-in skill parsing and override precedence regression tests.
