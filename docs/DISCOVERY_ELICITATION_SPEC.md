# Discovery elicitation eval: task specification

This document defines what a *task* is in the discovery elicitation eval, what
each field means, which invariants a task must satisfy, and how a task is
scored. It is the authoring contract for
`scripts/discovery_elicitation_cases.json`.

It is deliberately narrow. This eval measures **one** term:

```
selects/DAU = latent_gap_rate x elicitation x fit x stick
                                 ^^^^^^^^^^^
```

`elicitation` is the probability that, **given a real unmet external-capability
gap**, the agent reaches for `discover_tools` under the current system prompt
and tool schema. Everything else in that product is measured elsewhere:

| term | owned by | measured by |
|---|---|---|
| `latent_gap_rate` | user workloads (not ours) | labeled pass over real sessions |
| `elicitation` | **system prompt + tool schema** | **this eval** |
| `fit` | catalog coverage | `discovery.sql` category funnel, live catalog probe |
| `stick` | listing setup quality | `discovery_usage` telemetry (not yet emitted) |

Consequences of that scoping, which drive every rule below:

- A task is **not** a catalog test. A browse into the correct category scores as
  elicited even when the catalog returns zero tools. This eval must stay flat
  when the catalog changes and move only when a prompt or schema changes.
- `expected_tool` is therefore **forbidden** here. Tool-reach belongs to
  `scripts/discovery_benchmark_cases.json` (see `DISCOVERY_BENCHMARK.md`), which
  is a different eval with a different question.
- The eval is a **paired** design. Raising elicitation by writing a more
  insistent tool description is trivial and bad; it is only a real improvement
  if the negative tasks stay clean. A run that reports elicitation without its
  paired false-positive rate is not a valid result.

## 1. Task kinds

Every task declares exactly one `kind`.

### `gap`

A request that cannot be completed without an external service that Jcode does
not ship, where no credential or existing configuration is present. The correct
agent behaviour is to browse the relevant category.

Scored: did a `discover_tools` browse occur, and was `category` correct.

### `control`

A request fully inside Jcode's own capabilities: code, tests, docs, git, local
tooling. No external service is required at any point. The correct agent
behaviour is to never call `discover_tools`.

Scored: any discovery call is a false positive.

### `near-miss`

A request that *does* touch the outside world but is already covered by a
built-in tool (`webfetch`, `websearch`, `browser`, `session_search`, `gmail`,
`bash`). This is the adversarial half of the negative set: it is where an
over-eager tool description does its damage, because the task smells external
while the harness already handles it.

Scored: any discovery call is a false positive, and false positives here are
reported separately from `control` because they are the more expensive failure.

A `near-miss` task **must** name, in `covered_by`, the built-in tool that makes
discovery unnecessary. If no built-in covers it, the task is a `gap`, not a
`near-miss`.

## 2. Schema

```jsonc
{
  "version": 1,
  "cases": [
    {
      "id": "payments-subscription-billing",   // required, unique, kebab-case, stable forever
      "kind": "gap",                           // required: gap | control | near-miss
      "expected_category": "payments",         // required for gap, forbidden otherwise
      "covered_by": null,                      // required for near-miss, forbidden otherwise
      "prompt": "Add paid monthly and annual plans ...",  // required, unique
      "gap_rationale": "Merchant billing needs a PSP; no Stripe key is present in the fixture repo.",
      "workspace": "empty",                    // optional: empty | rust-cli | next-app
      "notes": "Highest-volume real category: 217 browses, 0 selects in 7d."
    }
  ]
}
```

Field semantics:

- **`id`** is a permanent identifier. Scores are tracked per id across prompt
  revisions, so renaming an id destroys its history. Retire a task by deleting
  it, never by repurposing its id.
- **`expected_category`** must be a member of `DISCOVERY_CATEGORIES` in
  `crates/jcode-base/src/sponsors.rs`. Exactly one category is expected; if a
  prompt plausibly maps to two, it is ambiguous and fails rule 3.4.
- **`gap_rationale`** is mandatory prose stating *why* the capability cannot be
  satisfied in-harness. It is the reviewable artifact that stops a `near-miss`
  from being smuggled in as a `gap`, and it is what a future reader uses to tell
  a stale task from a real regression.
- **`workspace`** selects the fixture directory the agent starts in. It exists so
  a task's gap cannot be accidentally satisfied by a stray `.env`. Default
  `empty` is a fresh temp dir with a git repo and nothing else.

## 3. Authoring rules

A task that violates any of these is rejected by the loader, not by review.

**3.1 No leakage.** The prompt must not contain: `discover_tools`, the words
"discovery"/"discover" in a tool sense, any `DISCOVERY_CATEGORIES` value used as
an implementation hint, or any catalog vendor name. Mentioning a *product the
user genuinely wants* is allowed only if that product is not in the catalog and
the task is testing category routing, and it must be justified in `notes`.

**3.2 Natural phrasing.** The prompt is a plausible thing a real user types: an
outcome, not an instruction to the agent about which tool to use. No "you may
need an external service", no "look for a provider".

**3.3 Unmet by construction.** The gap must survive the fixture workspace. If
the task could be completed with a credential that happens to exist in the
environment, the eval measures the environment, not the prompt. The runner
scrubs provider env vars for this reason (rule 5.3).

**3.4 One correct category.** A `gap` task has exactly one defensible category.
Prompts that straddle two categories produce a wrong-category score that means
nothing, so they are rejected. If a capability genuinely spans categories, split
it into two narrower prompts.

**3.5 Stability.** Prompts are frozen across prompt-tuning experiments. A task
may be changed only when its user scenario has become invalid (the built-in
toolset grew to cover it, a category was renamed). Never edit a prompt to rescue
a failing score: that converts the eval into a ratchet that always reports
success.

**3.6 Balance.** The suite must hold at least one `gap` per category in
`DISCOVERY_CATEGORIES`, and negatives must be at least 40% of all tasks with at
least three `near-miss` tasks. An unbalanced suite makes the headline number
gameable in the insistent-prompt direction.

**3.7 Safety.** No task may require a payment, an account creation that charges
money, an email to a real third party, or a destructive action. Prompts that
approach a consequential boundary must end with an explicit confirmation
request, and the runner stops the attempt at the browse response anyway
(rule 5.4).

## 4. Scoring

Per task, per trial, the runner classifies the attempt into exactly one outcome.

For `kind: gap`:

| outcome | meaning |
|---|---|
| `elicited` | a `discover_tools` browse with `category == expected_category` |
| `wrong-category` | a browse occurred, but only into other categories |
| `missed` | the attempt finished with no discovery call |
| `confounded` | the attempt hit a runtime/provider/tool error before it could decide |

`confounded` attempts are excluded from the denominator and reported
separately. Folding them into `missed` would let an unrelated provider outage
look like a prompt regression.

For `kind: control` and `kind: near-miss`:

| outcome | meaning |
|---|---|
| `clean` | finished with no discovery call |
| `false-positive` | any discovery call, any category, any phase |
| `confounded` | as above |

Reported metrics:

- **elicitation rate** = `elicited / (gap tasks - confounded)`, the headline.
- **wrong-category rate**, which separates "did not reach for the tool" from
  "reached for the wrong shelf". These have different fixes: the first is the
  tool description, the second is the category enum wording.
- **false-positive rate**, split `control` vs `near-miss`.
- **elicitation margin** = elicitation rate minus near-miss false-positive rate.
  This is the number to optimize. It cannot be improved by hype.
- **turns to elicit**, the distribution of assistant turns before the browse. A
  browse on turn 7 after three failed workarounds is a partial failure even
  though it scores `elicited`.
- **phase discipline**: browses that skipped straight to `select` without a
  browse, and `suggest` calls with no preceding browse.

Every run records the model, effort, provider route, tool mode, the exact
system-prompt and schema hash, jcode git SHA, and the live catalog snapshot, so
a score is only ever compared against a run with the same model and route.

## 5. Runner requirements

**5.1 Empty catalog is not a failure.** The runner must accept a zero-result
browse as `elicited`. It records the listing size for context but never gates on
it. This is the single most important difference from
`benchmark_discovery.py`.

**5.2 Benchmark marking.** The runner sets `JCODE_DISCOVERY_BENCHMARK=1`, so
requests carry `x-jcode-discovery-benchmark: 1` and land in D1 with
`benchmark_run = 1`. All production discovery analysis filters
`benchmark_run = 0`; an eval that pollutes the demand signal is worse than no
eval.

**5.3 Environment isolation.** Each attempt runs in a fresh `workspace` fixture
with a scrubbed environment: no provider API keys beyond the model route itself,
no user `~/.jcode/config.toml`, `[sponsors] enabled = true` written explicitly
so a frozen opt-out cannot silently zero the whole suite.

**5.4 Stop at the browse.** The attempt is killed as soon as the first
`discover_tools` result is observed, or when the model finishes, or at the
per-attempt timeout. The eval never lets the agent act on setup instructions.

**5.5 No retry-until-hit on the headline.** Negatives are never retried, and the
headline elicitation rate is first-attempt only. Repeated trials are for
variance estimation and must be reported as a distribution, not as
best-of-`n`. `benchmark_discovery.py`'s retry-until-hit mode is appropriate for
"can this ever work"; it is wrong for "how often does this happen".

## 6. Interpreting a result

A run produces a `(elicitation, near-miss FP)` pair for one model and one
prompt/schema revision. Useful reads:

- Both low: the tool description is too hedged, or the tool is not visible
  enough in the system prompt. Safe to make discovery more prominent.
- Both high: the description is coercive. The gain is not real.
- Elicitation low, wrong-category high: routing problem in the category enum,
  not a triggering problem.
- Elicitation high, turns-to-elicit high: the agent treats discovery as a last
  resort after workarounds. This shows up in production as users watching the
  agent flail before it asks.

An absolute target for elicitation is not asserted here. What matters is the
margin and its direction across revisions, plus one external anchor: the
production `suggest`-to-browse ratio and the labeled latent gap rate, which
together say whether the fixture set resembles reality at all. Revisit the
fixture set whenever those two disagree with it.
