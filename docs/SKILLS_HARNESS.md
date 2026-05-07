# Skills Harness

jcode skills are markdown instruction packs stored as `SKILL.md` files with YAML frontmatter. They can guide coding, review, refactoring, debugging, optimization, or other repeatable workflows.

## Skill sources and priority

Skills are loaded deterministically. Later sources override earlier sources with the same `name`:

1. built-in skills embedded in the binary;
2. project compatibility skills from `./.claude/skills`;
3. global jcode skills from `~/.jcode/skills`;
4. project-local jcode skills from `./.jcode/skills`.

This means a project-local `.jcode/skills/<name>/SKILL.md` can override a built-in skill without changing the binary.

## Built-in skills

This fork embeds:

- `karpathy-guidelines`, vendored from `forrestchang/andrej-karpathy-skills`;
- `optimization`, from this repository's existing `.jcode/skills/optimization` skill;
- `clean-code-guardian`, an original Clean Code inspired quality policy for coding, review, refactoring, and debugging;
- `llmwiki-memory`, an operational skill for using the local LLM wiki as durable project memory with provenance and secret-safety boundaries.

The built-ins are compiled with `include_str!`, so runtime skill loading does not require internet access, Node, Claude Code, Cursor, Codex CLI, or a plugin marketplace.

## CLI

```bash
jcode skills list
jcode skills list --json
jcode skills show karpathy-guidelines
jcode skills show karpathy-guidelines --json
jcode skills sync
jcode skills sync --force
jcode skills doctor
jcode skills doctor --json

jcode-harness skills list
jcode-harness skills list --json
jcode-harness skills show karpathy-guidelines
jcode-harness skills show karpathy-guidelines --json
jcode-harness skills sync
jcode-harness skills doctor
jcode-harness skills doctor --json
jcode-harness skills match "fix this Rust bug" --json
jcode-harness skills match "review this diff" --skill repo-reviewer --cwd /path/to/repo
jcode-harness skills llmwiki-bridge
jcode-harness skills llmwiki-bridge --json
```

Quality gate commands:

```bash
jcode clean-code check
jcode clean-code check src tests --fail-on warning
jcode clean-code rules
jcode-harness clean-code check --json
```

`sync` copies built-in skills to `~/.jcode/skills` and does not overwrite existing files unless `--force` is used.

`skills doctor` reports loaded skills, built-in availability, invalid frontmatter found while scanning standard paths, duplicate names across origins, and the final effective path for each loaded skill.

`skills list --json`, `skills show <name> --json`, and `skills doctor --json` provide stable machine-readable output for automation. JSON entries include skill `name`, `description`, `origin`, `path`, and `allowed_tools`; `show` also includes `content`; `doctor` includes `skills_loaded`, `builtins`, `duplicates`, and final effective `skills`. See `docs/JCODE_HARNESS_JSON_SCHEMAS.md` for the stable schema contract.

`skills match <goal>` previews task-scoped skill routing without invoking a provider. It uses the same deterministic router as `jcode-harness run`, includes explicit `--skill <name>` values first, and resolves effective skill metadata with the same source priority described above. Use `--cwd <repo>` to inspect repo-local `.jcode/skills` overrides from automation that is not already running inside the repository. This is the offline bridge for repo and task-level skill scoping.

`skills llmwiki-bridge` is a permission-reviewed bridge between the embedded `llmwiki-memory` skill and concrete local LLM wiki MCP commands. It is an offline preview only: it prints the supported command mapping, examples, read/write/secret boundaries, and recommended flow, but does not invoke MCP tools or require network access. Use `--json` when another harness wants a stable automation contract for `wiki_query`, `wiki_search`, `wiki_read_page`, `wiki_sync`, `wiki_export`, and `wiki_lint`.

## Harness run

```bash
jcode-harness
jcode-harness smoke
jcode-harness demo --json
jcode-harness demo run mock-provider-run-json --json
jcode-harness demo run all --sandbox --json
jcode-harness run "fix this Rust bug" --provider openai-compatible --model gpt-4.1
jcode-harness run "optimize memory usage" --skills always --dry-run
jcode-harness run "review this diff" --skill karpathy-guidelines --max-turns 3 --json
jcode-harness run "review this diff" --json --mock-response "deterministic response"
jcode-harness run "review this diff" --ndjson --mock-response "deterministic response"
```

`jcode-harness` with no subcommand starts the regular interactive jcode experience.

`jcode-harness run` uses the same provider initialization, tool registry, and `Agent` runtime as `jcode run`, while remaining script-friendly. It prepends selected skill context before starting the agent loop.

`--mock-response <text>` uses a deterministic local provider named `harness-mock`. It exercises the real `Agent` runtime, JSON/NDJSON output, session creation, usage reporting, and skill preface path without network access or provider credentials. This is intended for CI and contract smoke tests, not for production model calls.

## Reproducible demo harness

`jcode-harness demo --json` prints a deterministic manifest of offline demos that back product and README claims. The current manifest covers safe-eval, mock-provider, local memory bridge, plan/init scaffolding, swarm init scaffolding, browser-adjacent doctor diagnostics, skill routing, and release-gate smoke paths.

Use the runner when you want real evidence instead of a static command list:

```bash
jcode-harness demo run mock-provider-run-json --json
jcode-harness demo run all --json
jcode-harness demo run all --sandbox --json
```

Safety boundaries:

- The manifest command does not execute demos.
- The runner only executes commands from the deterministic local manifest.
- Demos that declare `project_writes: true` are blocked by default.
- `demo run all --json` executes non-writing demos and reports writing demos as blocked with `status: "warn"`.
- `demo run all --sandbox --json` executes all demos in a temporary workspace, reports `execution_root`/`executed_root`, and removes the sandbox by default.
- `--keep-sandbox` keeps generated files for manual inspection; `--allow-writes` should only be used in disposable or safe-eval workspaces.

### Opt-in live-provider smoke

Live-provider validation is intentionally excluded from default tests because it can use network, credentials, and paid provider quota. Run it only when you explicitly want to verify a real provider through `jcode-harness run`:

```bash
JCODE_HARNESS_LIVE_PROVIDER_SMOKE=1 \
JCODE_HARNESS_LIVE_PROVIDER=openai-api \
JCODE_HARNESS_LIVE_MODEL=gpt-4.1-mini \
cargo test --test e2e harness_live_provider -- --nocapture
```

Provider-profile based smoke is also supported when the profile is made available inside the isolated test environment:

```bash
JCODE_HARNESS_LIVE_PROVIDER_SMOKE=1 \
JCODE_HARNESS_LIVE_PROVIDER_PROFILE=my-reviewed-profile \
JCODE_HARNESS_LIVE_PROVIDER_CONFIG=/path/to/reviewed-config.toml \
JCODE_HARNESS_LIVE_MODEL=my-model \
cargo test --test e2e harness_live_provider -- --nocapture
```

Safety boundaries:

- The test skips unless `JCODE_HARNESS_LIVE_PROVIDER_SMOKE=1` is set.
- The subprocess receives an isolated temporary `JCODE_HOME`, `JCODE_RUNTIME_DIR`, and cwd.
- `JCODE_HARNESS_LIVE_PROVIDER_PROFILE` requires `JCODE_HARNESS_LIVE_PROVIDER_CONFIG`, which is copied to `config.toml` inside the isolated `JCODE_HOME`.
- Do not paste tokens into docs, prompts, command history, side panels, wiki pages, or committed fixtures.
- Prefer provider profiles that use `api_key_env`; avoid inline `api_key` values in copied configs.
- Prefer short, low-cost models and temporary environment-scoped credentials.
- The default CI/e2e path remains offline and credential-free.

## Skill router

The router is intentionally simple and deterministic:

- coding, bug, test, refactor, review, implement, fix, pull request, or diff tasks select `karpathy-guidelines` and `clean-code-guardian`;
- performance, latency, memory, throughput, CPU, RAM, or efficiency tasks select `optimization`;
- LLM wiki, project memory, prior decision, provenance, transcript, or context-history tasks select `llmwiki-memory`;
- explicit `--skill <name>` always includes that skill;
- `--skills off` disables automatic routing while preserving explicit `--skill` values;
- `--skills always` includes built-in coding and optimization skills.

The router does not inject every skill by default.
