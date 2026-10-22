# Harness refinement primitives (`jcode-plan::refine`)

Status: Implemented (primitives only; no runner, no CLI, no model calls).
Ported from harness2 (`google-research/harness2`, Apache-2.0, Google LLC).

## Why this exists

jcode is an *execution* harness: it runs agents well (swarm, memory, tools).
harness2 is a *meta* harness: it improves other harnesses per task and keeps
what worked. jcode has no equivalent loop, so every task starts from the same
static prompt/skills no matter what earlier tasks proved. These primitives
port the executor-independent half of harness2 into `jcode-plan` so a future
`jcode refine --k` runner can reuse validated logic instead of re-inventing
it. No executor, model call, network, or filesystem IO lives here: callers
supply trees as maps and persist what these helpers return. That keeps the
crate dependency-free (serde only) and fully unit-testable.

## What was ported, why, and the evidence

### 1. `surface`: editable c1..c8 component tree + validators
Source: `harness2/core/tree.py` (`SurfaceSpec`, `read_tree`, `validate`,
`autotrigger`, `diff_summary`, `diversity`).

Why: a harness is files with different runtime costs. Always-on prose is in
context on every run and competes with the agent's own output budget; skills,
scripts, subagents, and tool-loop hooks cost nothing until invoked. The
validators enforce the three rules that discard a whole proposal in
harness2's published runs: every skill needs a trigger line (an untriggered
skill is dead weight the agent never opens), no template braces in always-on
text (Jinja `StrictUndefined` kills the job pre-trial), every named script
must ship (a skill invoking a missing file fails validation).

Evidence: harness2 reports up to +11.5 pts on WorkBuddy-Bench, +4.5 on
JobBench, +11.8pp on LAB partial pass rate from this surface discipline; its
prompts name "no deliverable because the model exhausted its output budget on
reasoning" as the most costly failure mode, which the always-on caps
(24k OpenCode / 8k Codex) directly guard against.

Tests: 18 unit tests (malformed-path panic sweep, escape rejection,
allowed-surface coverage, skill-name rule on both surfaces, untriggered-skill
rejection, triggered-skill census, char-based caps, hooks-file schema
validation, autotrigger repair, skill-helper editability + 600-char window,
diff + diversity, script-ref scanning incl. quoted/period/flag forms,
missing/wrong-skill script rejection, shipped/skill-local acceptance,
Jinja-brace rejection).

### 2. `recursion`: task-level budgets, stage labels, finals-only pool
Source: `harness2/core/recursion.py` (`run_parallel`, `run_sequential`,
`ModeResult`, `selection_pool`).

Why: parallel recursion explores wider (2k candidates, k compositions);
sequential refines deeper (k passes carrying the refined harness forward).
The label convention (`inv{i}`/`dom{i}`/`agg{i}` vs `inv{t}`/`dom{t}`/`c{t}`),
the 4k-rollout / 3k-stage budget math, and the finals-only pool (reference
executions excluded, no abstention by construction) are the published
contract a future runner must honor to reproduce harness2 results. `kept_base`
encodes KEEP_BASE as a real answer, not a failure.

Evidence: harness2's experiment guide defines `--k`/`--mode` semantics and
shows further gains from larger task-level budgets; the pool convention is
called "the published convention and the only pool this release builds".

Tests: 6 unit tests (label conventions, k=3 budget math, budget rejection,
finals-only pool, kept-base detection, mode token roundtrip).

### 3. `selection`: anonymized choice with safe fallbacks
Source: `harness2/core/selection.py` (`assign_ids`, `_render_pool`,
`run_selection`, `trivial_selection`, `has_experience` as `Experience::new`).

Why: anonymized outputs (A..Z via seeded shuffle, stable across reruns) mean
no credit for picking a more elaborate candidate and no privileged "final"
candidate; picking an unmodified harness's output is valid and expected.
Non-answers are dropped with causes before anonymising. A selector failure
falls back to a delivered base, else the first final: it never raises, so one
bad judgement cannot lose the whole task.

Evidence: harness2's selector prompts state candidates "are anonymous on
purpose" and "picking an output from an unmodified harness is a completely
valid and expected outcome"; failed evaluations "remain unscored rather than
being counted as zero".

Tests: 7 unit tests (anon-id counting + 2000-id collision sweep, shuffle
stability + coverage, non-answer drops with `label (cause)` entries,
choice/base/final fallback chain with carried anon ids, k=1
`trivial_selection` fast path + empty-pool rejection, experience field
gating, adversarial normalize sweep).
Note: seed uses FNV-1a + SplitMix64 instead of sha1 so the crate
stays dependency-free; the stability property (same input, same shuffle) is
what the tests pin, not the hash function.

### 4. `library`: per-domain experience library
Source: `harness2/core/library.py` (`Library.add`, `rebuild_index`,
`root_for`, `INDEX.md` / `EXPERIENCE.md` layout).

Why: this is the harnessing-level recursion half. Without it every task
starts from base; with it, later tasks in a domain read full adopted trees
(never summaries) plus the cue/lesson/evidence triple as examples. Tasks
within a domain run ordered and read only earlier entries; domains run
concurrently. The `<base>/<benchmark>/k<k>-<variant>` layout keeps
experiments distinguishable; an explicit tag opts into sharing.

Evidence: harness2's method section: "after adoption, reflection stores the
adopted harness and evidence-supported lessons in a per-domain library";
retrieval is by construction (a domain dir only holds same-domain entries).

Tests: 5 unit tests (slugs, domain dirs incl. whitespace-edge parity,
inventory census, experience-md with why + edits + lesson, index with
per-entry inventory + empty state, root layout incl. tag override).

### 5. `hygiene`: eval-hygiene guards
Source: `harness2/core/context.py` (grader-verdict tripwire,
`assert_no_leak_paths`) + `harness2/core/spec.py` (`BenchPrompts`
validation) + `EXPERIMENTS.md` web-access section + `BenchSpec.obs_trunc`
floor.

Why: four rules, each from a real published failure. (a) Prompt slots must
describe the WORK, not its measurement (benchmark/evaluation/grader/rubric/
score rejected): a prompt that tells the agent what the grader wants is
leakage, not improvement. (b) Grader-verdict tripwire on evidence trees:
the grader's own output markers (`deterministic_eval_summary`,
`"pass_rate"`, judge findings, ...) are refused by file name AND by content
(scanned suffixes only), with the match offset quoted so the read path can
be found. If it fires, find the leak; do not widen around it. (c) Leak-path
rejection with true Python-fnmatch semantics (verified: `*`/`?`/`[...]`
span separators): answer-key paths are rejected before a context reaches a
model; plus the egress blocklist (`huggingface.co`, `github.com`, ...) for
eval runs, after the published incident where a Codex rollout downloaded its
own task's `RUBRICS.json` unprompted. (d) `obs_trunc` floor 10_000: below
that the renderer *fabricates* defects; default 50_000.

Tests: 10 unit tests (prompt-slot words + alias, marker-content tripwire,
marker-in-name, suffix case-insensitivity + name-tripwire, unscanned-suffix
skip, floor rejection, fnmatch glob semantics verified against CPython +
class/negation/empty/Unicode edge cases, wildcard bounded-time proof,
leak-assert hits, blocklist coverage).

## What was deliberately NOT ported

- Executors (`substrate/opencode.py`, `substrate/codex.py`), model channels,
  prompts, judging/reporting CLIs: these are harness2's runtime, not
  primitives, and jcode already owns its execution layer.
- The `jcode refine` runner itself: proposed follow-up using these
  primitives; this PR is the foundation, kept reviewable.

## Validation

- `cargo test -p jcode-plan`: 69 passed, 0 failed (23 pre-existing: 13 plan + 10 mermaid; 46 new: surface 18, recursion 6, selection 7, library 5, hygiene 10).
- `cargo clippy -p jcode-plan --all-targets`: no warnings from this change.
- `cargo fmt -p jcode-plan`: clean.
- `cargo check -p jcode-plan -p jcode-swarm-core -p jcode-protocol`: clean
  (all downstream consumers of `jcode-plan`).

## CI integration verification (2026-10-22)

The PR's merge-result CI exposed an integration failure hidden by testing its
older branch alone: upstream commit `774613c90` removed `serde_json` from
`jcode-plan`, while refinement's hooks validator now requires it. The branch
merges master `59fa887c8dd9bb76f8216012e3ed5eaf1ab20f0f` and explicitly restores
the dependency and lockfile edge. The unused `glob` and `ignore` app-core
dependency declarations are removed, not exempted from the dependency audit.

Four quality ratchets also fail on a pristine archive of that same master.
Their baseline reconciliation is deliberately a separate commit, with the JSON
generated **only from the pristine upstream archive**, not the PR source. Each
of the four committed JSON files is byte-identical to that upstream-only result.
This acknowledges inherited master debt, including increases, rather than
claiming it was cleaned up. Thresholds, scanners, and CI checks are unchanged.
It grants no allowance to the new refinement files. To reproduce the baseline
provenance, archive the pinned master into an empty scratch directory, run its
four `scripts/check_{code_size,test_size,panic,swallowed_error}_budget.py` scripts
with `--update` there, and compare each generated JSON to this branch's file.

New-code violations are fixed instead: review regressions live in public-API
integration tests rather than pushing `surface.rs` over 1,200 lines, a missing
trigger file is explicitly created through the map entry API, and an empty
selection pool returns an error rather than manufacturing an empty label.
The documented unmapped-anonymous-ID case remains an explicit empty ID.

Observed local results: all 72 plan tests pass (69 unit, 3 integration), strict
plan Clippy passes, the exact CI dependency auditor `cargo-machete 0.9.2` reports
no unused dependencies, app-core compiles, all four reconciled ratchets pass,
and dependency boundaries, module resolution, wildcard exports, workspace
formatting, and locked Cargo metadata pass. Hosted builds and review approval
must still be checked independently before claiming merge readiness.
