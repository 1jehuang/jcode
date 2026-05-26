# JBench

JBench is jcode's evaluation framework for measuring AI coding agent
performance through real-world git commit reconstruction tasks. It is the
Rust port and adaptation of [Codebuff's BuffBench](https://github.com/codebuff/codebuff/tree/main/evals/buffbench)
to the jcode multi-agent foundation.

> **Status: scaffolding.** This crate currently provides typed data
> models, module skeletons, and a CLI shell. The actual eval
> orchestration (cloning repos, spawning agents, calling judge models,
> running lessons extraction) is intentionally left as `unimplemented!()`
> stubs so reviewers can validate the shape of the public API before any
> end-to-end behavior lands. Real implementations will arrive in Phases
> 5.3 (`agent_runner`), 5.4 (`judge`), and 5.5 (`lessons`).

## Why git commit reconstruction?

The core idea, borrowed directly from BuffBench, is that real git history
contains a near-infinite stream of well-scoped, naturally-occurring tasks
with built-in ground truth: each commit is a self-contained change with a
known intent (the message / spec) and a known correct outcome (the diff).

For each evaluation:

1. Pick a commit `C` from a target repository.
2. Reset the working tree to `parent(C)`.
3. Hand the agent a natural-language prompt derived from `C`'s spec.
4. Let the agent edit the repo.
5. Compare the agent's diff against the ground-truth diff in `C`.

This yields fair head-to-head comparisons across agents because every
agent works from the exact same starting state and is judged against the
same target.

## Three-judge median

A single LLM judge is noisy. JBench follows BuffBench's approach: every
agent diff is judged by **three** different frontier models in parallel
(today the planned slate is `gpt-5`, `gemini-pro`, and `claude-sonnet`),
and the median `overall_score` is reported as the canonical result. Per-
dimension averages (`completion_score`, `code_quality_score`,
`overall_score`) are reported alongside the median's qualitative
analysis.

The three-judge pipeline lives in `src/judge.rs` (currently
`unimplemented!()`). See `/tmp/codebuff/evals/buffbench/judge.ts` for the
TypeScript original we are mirroring.

## Lessons extractor

After each run, the lessons extractor compares the agent's diff and
trace against the ground-truth diff and emits a small list of
`Lesson { what_went_wrong, what_should_have_been_done }` items. These
lessons are intended to be appended to per-agent lesson files that can
later be folded into the agent's system prompt or memory graph — the
classic "learn from your mistakes" loop.

The lessons module lives in `src/lessons.rs`.

## Reuse of `jcode-agent-runtime`

JBench is built on top of the new agent foundation in
[`crates/jcode-agent-runtime`](../../crates/jcode-agent-runtime/), which
provides:

- `AgentRegistry` — discovery and loading of `.jcode/agents/*.toml`
  agent definitions.
- `AgentDefinition` — the declarative schema describing an agent's
  model, tools, system prompt, output mode, etc.

The agent runner (`src/agent_runner.rs`) will resolve agent IDs against
the registry, spawn a `jcode` subprocess in a clean clone of the target
repo, capture the trace, and return an `EvalRun` populated with the diff
and judging result.

## Module map

| Module | Purpose |
| --- | --- |
| `types` | Serializable data structures (`EvalCommit`, `FileDiff`, `EvalDataV2`, `EvalRun`, `JudgingResult`, `AgentEvalResults`). Roundtrip-tested. |
| `judge` | Three-judge median pipeline. **Stub.** |
| `agent_runner` | Spawn an agent in a repo, capture trace + diff. **Stub.** |
| `lessons` | Extract lessons from a failed/imperfect run. **Stub.** |
| `bin/jbench.rs` | CLI: `pick-commits`, `gen-evals`, `run`, `judge`, `meta-analyze`. Each subcommand currently prints a TODO and exits 0. |

## Workflow (planned)

```
pick-commits   →  select high-quality commits from a repo
gen-evals      →  produce eval-{repo}.json with EvalDataV2 schema
run            →  run agents against eval data, emit EvalRun per commit
judge          →  re-judge an existing run with the 3-model median
meta-analyze   →  aggregate analysis across all tasks for an agent
```

## Running

```bash
cargo check -p jcode-jbench
cargo test  -p jcode-jbench
cargo run   -p jcode-jbench --bin jbench -- run --help
```

## References

- BuffBench source: `/tmp/codebuff/evals/buffbench/`
- BuffBench README: `/tmp/codebuff/evals/buffbench/README.md`
- Judge design: `/tmp/codebuff/evals/buffbench/judge.ts`
- Agent runner design: `/tmp/codebuff/evals/buffbench/agent-runner.ts`
- Lessons extractor design: `/tmp/codebuff/evals/buffbench/lessons-extractor.ts`
