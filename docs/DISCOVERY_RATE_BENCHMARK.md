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
named baselines.

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
