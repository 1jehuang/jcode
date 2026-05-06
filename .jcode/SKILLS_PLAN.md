# Skills Plan

## Recommended initial skills

Use skills by task rather than injecting every skill into every prompt.

- `rust`: default for implementation, review, and test work in this Rust 2024 workspace.
- `karpathy-guidelines`: use for concise engineering judgment, code review, and repo hygiene.
- `optimization`: use for performance, compile-time, memory, multi-session scaling, and root-crate fan-out work.
- `clean-code-guardian`: use when touching production code or preparing release gates, especially with offline `clean-code check` validation.

## Project-specific notes

- Built-in skills must remain usable offline without runtime network access, Node, Claude Code, Cursor, Codex CLI, or plugin marketplaces. Evidence: `AGENTS.md` embedded skills guardrails.
- Preserve vendored attribution under `third_party/andrej-karpathy-skills/` and `NOTICE.md` when updating `karpathy-guidelines`. Evidence: `AGENTS.md`.
- Keep `clean-code-guardian` original and do not vendor copyrighted Clean Code text. Evidence: `AGENTS.md`.
- Release gates require deterministic skill precedence, JSON output, duplicate-skill diagnostics, and e2e harness CLI coverage. Evidence: `docs/JCODE_HARNESS_RELEASE_GATES.md`.

## Evidence-backed validation candidates

```bash
cargo test -p jcode skill::tests --lib
cargo test --test e2e harness_cli -- --nocapture
cargo run -q -p jcode --bin jcode-harness -- skills list --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills doctor --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- clean-code rules >/tmp/jcode-clean-code-rules.yaml
```
