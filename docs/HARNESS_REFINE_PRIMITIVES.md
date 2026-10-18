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

Tests: 11 unit tests (escape rejection, allowed-surface coverage,
untriggered-skill rejection, triggered-skill census, cap + non-empty
enforcement, autotrigger repair, diff + diversity, script-ref scanning,
missing-script rejection, shipped/skill-local acceptance, Jinja-brace
rejection).

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

Tests: 5 unit tests (label conventions, budget rejection, finals-only pool,
kept-base detection, mode token roundtrip).

### 3. `selection`: anonymized choice with safe fallbacks
Source: `harness2/core/selection.py` (`assign_ids`, `_render_pool`,
`run_selection`, `trivial_selection`, `has_experience`).

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

Tests: 5 unit tests (anon-id counting, shuffle stability + coverage,
non-answer drops, choice/base/final fallback chain, experience field
gating). Note: seed uses FNV-1a + SplitMix64 instead of sha1 so the crate
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

Tests: 5 unit tests (slugs, domain dirs, inventory census, experience-md
content, index empty/populated, root layout incl. tag override).

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

Tests: 8 unit tests (prompt-slot words + alias, marker-content tripwire,
marker-in-name, unscanned-suffix skip, floor rejection, fnmatch glob
semantics verified against CPython, leak-assert hits, blocklist coverage).

## What was deliberately NOT ported

- Executors (`substrate/opencode.py`, `substrate/codex.py`), model channels,
  prompts, judging/reporting CLIs: these are harness2's runtime, not
  primitives, and jcode already owns its execution layer.
- The `jcode refine` runner itself: proposed follow-up using these
  primitives; this PR is the foundation, kept reviewable.

## Validation

- `cargo test -p jcode-plan`: 57 passed, 0 failed (27 pre-existing + 30 new).
- `cargo clippy -p jcode-plan --all-targets`: no warnings from this change.
- `cargo fmt -p jcode-plan`: clean.
- `cargo check -p jcode-plan -p jcode-swarm-core -p jcode-protocol`: clean
  (all downstream consumers of `jcode-plan`).
