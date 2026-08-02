# Discovery call-rate benchmark

`scripts/benchmark_discovery_rate.py` measures the policy we actually want to
hold: **the agent calls `discover_tools` whenever it reaches for an external
product, service, API, or data source, and it commits to a specific vendor
through `action=select` rather than around Discovery.**

This is a different question from `docs/DISCOVERY_BENCHMARK.md`. That benchmark
is catalog-locked: it verifies that each live listing is reachable from a natural
prompt. This one is catalog-independent and measures trigger behavior across a
broad suite, including tasks where triggering would be wrong.

## Run it

```bash
python scripts/benchmark_discovery_rate.py --provider jcode --model claude-haiku-4-5-20251001
python scripts/benchmark_discovery_rate.py --trials 3                 # tighter confidence
python scripts/benchmark_discovery_rate.py --tag control              # precision only
python scripts/benchmark_discovery_rate.py --case storage-user-uploads
python scripts/benchmark_discovery_rate.py --list                     # inspect the suite
```

Offline tests for the scoring and detection logic, no model credits needed:

```bash
python scripts/test_benchmark_discovery_rate.py
```

Reports land in `target/discovery-rate/latest.json`; use `--output` to keep
named baselines.

## The suite

`scripts/discovery_rate_cases.json` holds two kinds of case.

- `expect: "call"` — a task that genuinely needs an external capability. There is
  at least one per Discovery category, plus two open-category tasks (SMS, speech
  to text) where no category is asserted. These measure **recall**.
- `expect: "no-call"` — a nearby task that is purely local: refactoring, tests,
  writing copy, a Dockerfile, local SQLite. Any Discovery call here is a false
  positive. These measure **precision**, so recall cannot be bought by calling
  Discovery on everything.

Loading the suite enforces that prompts never name `discover_tools`, never say
"discovery", and never contain a category slug. A prompt that leaks the
mechanism measures nothing.

## Metrics

Per case and in aggregate:

- **browse rate** — fraction of trials that reached a `discover_tools` browse
  response. This is the headline recall number.
- **any-call rate** — any Discovery call, including a select without a browse.
- **bypass rate** — trials where the agent committed to an external product with
  no Discovery call at all: installing a vendor SDK, driving a vendor CLI,
  fetching a vendor API or signup page, or connecting an MCP server directly.
  A high bypass rate is the specific failure this benchmark exists to catch.
- **select rate** — trials that reached `action=select`, the second half of the
  intended policy.
- **category accuracy** — when a browse happened, whether it used the expected
  category.
- **control clean rate** — controls that finished with no Discovery call.

The run passes when aggregate browse recall clears `--min-recall` (default 0.8)
and control clean rate clears `--min-precision` (default 0.9).

## Bypass detection

Bypasses are matched against the agent's **tool input only**, never tool output.
Scanning output produced false positives: probing a workspace echoes vendor names
the agent never chose. Vendor CLI patterns are anchored to a command position, so
a vendor name inside a heredoc, a file path, or a `command -v` probe list does not
count. `scripts/test_benchmark_discovery_rate.py` pins both directions with
positive and negative fixtures; extend it whenever a pattern changes.

## Trial validity

A trial that never reached the model says nothing about triggering. When an
attempt produces no tool activity and dies with an auth, quota, billing, cost
ceiling, rate limit, or connectivity error, it is marked `invalid` and excluded
from every rate. Reports carry `scored_trial_count` and `invalid_trial_count`,
and a run with nothing scored can never pass. Without this, a logged-out provider
reports a perfect 0% trigger rate.

Each trial also gets a pristine workspace. A shared directory let files written
by one case prime later cases, which both leaks the answer and misattributes
bypasses.

## Benchmark traffic marking

Like the catalog benchmark, the runner starts a dedicated server with
`JCODE_DISCOVERY_BENCHMARK=1`, so every request carries
`x-jcode-discovery-benchmark: 1` and telemetry carries `benchmark_run: true`.
Benchmark traffic must be excluded from sponsor, billing, and organic-usage
reporting.

## Interpreting results

The trigger policy lives entirely in the `discover_tools` schema and description.
`jcode-base/src/prompt_tests.rs` asserts Discovery is never injected into the
system prompt, so the tool description is the only lever. When recall is low and
bypass is high, the fix belongs in that description, and this benchmark is the
feedback loop for it. Record a baseline before changing wording:

```bash
python scripts/benchmark_discovery_rate.py --output target/discovery-rate/before.json
# edit the discover_tools description
python scripts/benchmark_discovery_rate.py --output target/discovery-rate/after.json
```

Prompts are held fixed across such experiments. Change a case only when its user
scenario is invalid, never to rescue a score.

## Measured findings

Baselines collected while building this benchmark, all on the default full
toolset so Discovery competes with bash, browser, and web tools:

| model | scored trials | browse recall | bypass | select |
| --- | --- | --- | --- | --- |
| claude-haiku-4-5 | 11 | 18% | 45% | 0% |
| glm-4.7-flash | 12 | 38% | 0% | 0% |
| gpt-oss-120b (cerebras) | 24 | 0% | 11% | 0% |
| gemini-2.5-flash-lite | 9 | 44% | 0% | 0% |

Three things stand out.

**Triggering is strongly model-dependent.** gpt-oss-120b never reached for
Discovery on any case; it wrote application code instead. A weak model can score
0% for reasons no wording change will fix, so a description experiment is only
meaningful when both arms use the same model and that model calls Discovery at
least sometimes on the baseline.

**Select rate is 0% everywhere.** Not one trial across any model reached
`action=select`. Agents that browse tend to summarize the listing for the user
and stop. This is the larger half of the gap: the intended policy is browse then
select, and the second half never happens today.

**Bypass is the dominant failure mode on capable models.** claude-haiku wired up
vendor CLIs and SDKs in 45% of trials without a single Discovery call.

### What changed as a result

Two fixes landed against these numbers.

The tool description now names the concrete moments to browse (before installing
a vendor SDK or CLI, before writing vendor API calls or config, before fetching
vendor docs or pricing, before connecting an MCP server, before recommending a
provider), states the select obligation, and draws negative scope so local work
does not trigger it.

More importantly, the browse listing no longer prints each entry's setup
instructions. That was the direct cause of the 0% select rate: browse already
handed the agent everything it needed, so the second half of browse-then-select
had no purpose. Setup now lives only in the select response.
`scripts/verify_discovery_select.py` verifies that handoff end to end against a
local fake catalog, with no model credits and no live endpoint:

```bash
python scripts/verify_discovery_select.py ./target/selfdev/jcode
```

The description change has not yet been confirmed by a matched live run. Every
provider available during this work either exhausted its budget or throttled;
the harness reports such trials as `invalid` rather than scoring them, so the
attempted comparisons produced no usable signal.

The one usable pre-change arm is preserved in the repo at
`docs/discovery-baselines/flash-lite-before.json` (gemini-2.5-flash-lite, 9
scored trials, 44% browse recall, 0% select; per-trial transcripts trimmed). Because that arm is already measured,
finishing the comparison only needs the post-change arm, which halves the quota
cost:

```bash
JCODE_BIN=<after-bin> python scripts/benchmark_discovery_rate.py \
  --provider gemini-api --model gemini-2.5-flash-lite --trials 3 \
  --case storage-user-uploads --case authentication-signin \
  --case observability-traces --case analytics-product-funnel \
  --case code-review-automation --case web-search-live-answers \
  --case control-sqlite-local --case control-regex-debug \
  --output target/discovery-rate/flash-lite-after.json
```

Compare `summary.recall_browse_rate` and `summary.select_rate` against the
preserved before arm, and check `scored_trial_count` on both before drawing any
conclusion. Free-tier Gemini quotas reset daily; a full two-arm run exhausts
them, so run one arm per day.

Single-trial runs are noise. An early 12-case comparison moved any-call from 38%
to 25% with no consistent per-case pattern; at n=1 per case that difference is
not a signal. Use `--trials 3` or more, and read `scored_trial_count` before
trusting any number.
