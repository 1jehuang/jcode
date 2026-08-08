# Discovery call-rate benchmark

`scripts/benchmark_discovery_rate.py` measures the policy we actually want to
hold: **the agent calls `integration_tools` whenever it reaches for an external
product, service, API, or data source, and it commits to a specific vendor
through `action=select` rather than around the integration directory.**

This is a different question from `docs/DISCOVERY_BENCHMARK.md`. That benchmark
is catalog-locked: it verifies that each live listing is reachable from a natural
prompt. This one is catalog-independent and measures trigger behavior across a
broad suite, including tasks where triggering would be wrong.

## Run it

```bash
python scripts/benchmark_discovery_rate.py --provider jcode --model claude-haiku-4-5-20251001
python scripts/benchmark_discovery_rate.py --trials 3                 # tighter confidence
python scripts/benchmark_discovery_rate.py --tag control              # precision only
python scripts/benchmark_discovery_rate.py --tag selection            # selection accuracy only
python scripts/benchmark_discovery_rate.py --case storage-user-uploads
python scripts/benchmark_discovery_rate.py --list                     # inspect the suite
```

Offline tests for the scoring and detection logic, no model credits needed:

```bash
python scripts/test_benchmark_discovery_rate.py
```

Reports land in `target/discovery-rate/latest.json`; use `--output` to keep
named baselines. Every non-list report fingerprints the exact executable before
starting any trial. `config.executable` records the original command, resolved
path, `--version` output, embedded commit, SHA-256, and size. The runner pins the
resolved path for every trial so a symlink update cannot make later trials use a
different binary than the one named by the artifact.
This provenance contract is report schema version 2. Historical version 1
artifacts predate executable fingerprinting and must not be used as evidence for
which binary produced a result.

## The suite

`scripts/discovery_rate_cases.json` holds three kinds of case.

- `expect: "call"` - a task that genuinely needs an external capability. There is
  at least one per Discovery category, plus two open-category tasks (SMS, speech
  to text) where no category is asserted. These measure **recall**.
- `expect: "no-call"` - a nearby task that is purely local: refactoring, tests,
  writing copy, a Dockerfile, local SQLite. Any Discovery call here is a false
  positive. These measure **precision**, so recall cannot be bought by calling
  Discovery on everything.
- `expect: "select"` - a task where the user has already chosen a named product.
  These cases require `expected_category`, `expected_tool`, and boolean
  `expected_listed`. The checked-in suite covers context.dev as a listed product
  and Firecrawl as an off-catalog product. These measure **selection accuracy**.

Loading the suite enforces that prompts never name `integration_tools`, never say
"discovery", and never contain a category slug. A prompt that leaks the
mechanism measures nothing. Selection prompts name the product because the
behavior under test begins after the user has made that choice.

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
- **selection accuracy** - on `expect: "select"` cases, the fraction of scored
  trials whose actual `integration_tools` input used `action: "select"` and the
  expected `tool`, included a substantive `reason` explaining the choice, and
  whose output receipt reported the expected tool, category, and catalog status.
  Catalog receipts such as `Selected 'context.dev' from
  'web-data' ...` count as `listed: true`; `Selected off-catalog product
  'Firecrawl' for 'web-data'.` counts as `listed: false`. Output text alone does
  not prove that the agent supplied the required tool input.
- **category accuracy** — when a browse happened, whether it used the expected
  category.
- **control clean rate** — controls that finished with no Discovery call.

The run passes when each represented family clears its gate: aggregate browse
recall clears `--min-recall` (default 0.8), control clean rate clears
`--min-precision` (default 0.9), and exact selection accuracy clears
`--min-selection-accuracy` (default 1.0). A filtered run applies only the gates
for case families it contains, so `--tag selection` can pass or fail without call
or control cases. A run with no scored trials never passes.

Every trial stops on its first selection, whether correct or incorrect. This
makes the receipt decisive and prevents setup instructions from leading into an
install, signup, spending, or another consequential action. Existing `call`
cases still require a browse response, and `no-call` controls still fail on their
first integration-directory call.

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

Run this against the models people actually use, primarily Claude and GPT-5.6.
Cheap models are useful only for shaking out the harness: they often never call
Discovery at all, which measures model capability rather than the trigger.

Baselines collected while building this benchmark, all on the default full
toolset so Discovery competes with bash, browser, and web tools:

| model | scored trials | browse recall | bypass | select |
| --- | --- | --- | --- | --- |
| claude-haiku-4-5 | 11 | 18% | 45% | 0% |
| glm-4.7-flash | 12 | 38% | 0% | 0% |
| gpt-oss-120b (cerebras) | 24 | 0% | 11% | 0% |
| gemini-2.5-flash-lite | 9 | 44% | 0% | 0% |
| **claude-fable-5** | **24** | **83%** | **11%** | **0%** |

The Claude row is the one to trust: 24 scored trials, zero invalid, and 100%
control precision. It reframes the problem.

Three things stand out.

**On a capable model the browse trigger already mostly works.** Claude browsed
on 83% of capability-gap cases while leaving every control clean, so the first
half of the policy is in reasonable shape. Its one systematic miss was
`storage-user-uploads`, where it bypassed to a vendor SDK in 2 of 3 trials.

**Installed skills can preempt Discovery.** Claude's only systematic miss was
`storage-user-uploads`. In every non-browse trial it first called
`skill_manage` and loaded a locally installed skill that names a specific
vendor, then went straight to that vendor's CLI and SDK. Discovery never got a
chance. Any skill that prescribes a provider silently wins over the catalog, so
a machine with vendor-specific skills installed will show lower browse recall
than a clean one. Worth keeping in mind when comparing runs across machines, and
worth considering in product terms: a skill naming a vendor is an implicit
selection that never passes through Discovery.

**Triggering is strongly model-dependent.** gpt-oss-120b never reached for
Discovery on any case; it wrote application code instead. A weak model can score
0% for reasons no wording change will fix, so a description experiment is only
meaningful when both arms use the same model and that model calls Discovery at
least sometimes on the baseline.

**Select rate is 0% everywhere, including Claude.** Not one trial across any
model reached `action=select`, even when Claude browsed successfully on 20 of 24
trials. Agents that browse summarize the listing and stop. This, not the browse
trigger, is the real gap: the intended policy is browse then select, and on the
strongest model tested the second half never happened once.

**Bypass is the dominant failure mode on capable models.** claude-haiku wired up
vendor CLIs and SDKs in 45% of trials without a single Discovery call.

### Matched before/after on Claude

Same model, same eight cases, three trials each, 24 scored trials per arm with
zero invalid. Preserved in `docs/discovery-baselines/claude-fable-5-{before,after}.json`.

| metric | before | after |
| --- | --- | --- |
| browse recall | 83% | **100%** |
| bypass | 11% | **0%** |
| control clean rate | 100% | 100% |

The entire gain came from `storage-user-uploads`, the one case that failed
before: browse 0% to 100%, bypass 67% to 0%. Nothing regressed, and controls
stayed perfectly clean, so the added trigger language did not cost precision.

**Select needs a populated catalog.** Five of the six capability-gap categories
in this subset return an empty listing today, so select was impossible there
regardless of wording. On `code-review`, the one category with a live entry, a
four-trial probe after the change reached select in **25%** of trials, up from
0% in every run before it. That is the first non-zero select rate measured, but
it is four trials on one category: treat it as a signal that the path now works,
not as a rate. Re-measure once more categories carry listings.

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

That script also covers off-catalog selects. The endpoint signals "no such
entry" either with a 404 or with a 200 carrying an empty entry; both are
reported to the agent as a distinct, actionable error naming `action=suggest`,
rather than as a generic endpoint failure it might retry or route around. The
same distinction is recorded in telemetry as
`outcome=off_catalog_select` / `failure_reason=off_catalog_select`.

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
