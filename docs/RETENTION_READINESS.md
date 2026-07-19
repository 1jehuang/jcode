# Retention readiness

jcode separates two questions that should not share a label:

1. **Retention readiness**: deterministic product properties that make returning useful and cheap.
2. **Observed retention**: whether real, privacy-safe cohorts actually return and complete meaningful work.

The first is a regression test. The second is a product outcome. A deterministic test must never be presented as D1, D7, or D30 user retention.

## Deterministic synthetic cohort

Run:

```bash
cargo test -p jcode-app-core --lib retention_readiness -- --nocapture
```

The evaluator drives the real `Agent`, provider interface, persisted `Session`, and disk restore path through labeled phase boundaries:

| Boundary | Journey |
|---|---|
| D0 | Send one activation prompt and require useful first value plus a durable session. |
| D1 | Create a new Agent and provider registry from the persisted session. Continue without restating context. Verify transcript, title, working directory, and memory marker. |
| D7 | Create another Agent and provider registry, then inject one provider outage. Require the failed D7 prompt itself to persist, then recover in one retry. The final result is only returned when both D0 and D1 context are present. |

D0/D1/D7 are deterministic phase labels, not wall-clock sleeps.

### Dimensions

| Dimension | Weight | Raw evidence |
|---|---:|---|
| Activation / first value | 25% | Useful D0 response and persisted session |
| Return friction | 20% | Context-restatement steps and prompts-to-value |
| Continuity | 20% | Prior transcript, title, workspace, and memory marker survive disk rehydrate |
| Durability | 15% | Session remains loadable and history survives the injected failure |
| Failure recovery | 10% | Outage surfaces and returns to useful value in one retry |
| Compounding value | 10% | D7 success requires context from both prior boundaries |

The behavioral score is a weighted geometric mean:

```text
behavioral = 100 * product((dimension_score / 100) ^ weight)
```

A zero dimension therefore collapses the behavioral score instead of being hidden by unrelated perfect dimensions. Critical behaviors also have direct assertions.

## Evidence coverage

The scorer keeps an explicit factor registry:

- **Scored (6)**: response first value, return friction, continuity, restart/failure durability, recovery, compounding context.
- **Deferred (2)**: tool/file-edit-backed first value and persisted credential/provider reconstruction across the provider-by-OS matrix.
- **Observed only (1)**: real D1/D7/D30 meaningful-work retention. This is excluded from the deterministic denominator.

Current deterministic result:

```text
Behavioral readiness: 100 / 100
Evidence coverage:      75 / 100 (6/8 deterministic factors)
Coverage-adjusted:      75 / 100
```

The honest headline is **75/100 coverage-adjusted retention readiness**, not “100% retention.”

The fixture rebuilds the provider registry at every phase, but it deliberately does not treat a fixture provider as proof that production credentials survive a restart. Authentication continuity remains part of the deferred provider-by-OS matrix.

Installation, PATH, upgrade, and uninstall durability are separately exercised by `scripts/setup_friction_eval.sh`. They should remain separate scorecards so a broad install suite cannot drown out conversation continuity failures.

## Observed counterpart

Actual retention should use cohorts activated by meaningful work, not raw launches:

```text
D7 meaningful-work retention =
  activated users with >=1 successful assistant/tool outcome in the D7 window
  -----------------------------------------------------------------------------
  users whose first successful assistant/tool outcome occurred on D0
```

Report at least:

- D1, D7, and D30 return after first successful assistant response
- D1, D7, and D30 return after first successful tool or file edit
- repeat successful workflows per active user
- return after upgrade
- authentication-recovery success followed by meaningful work

jcode already records coarse active-day helpers (`active_days_7d` and `active_days_30d`). Those are useful engagement signals, but they are not cohort retention by themselves because they do not anchor the denominator to an activation event or require a meaningful successful outcome.

## Calibration loop

1. Find a deterministic factor regression or weakness.
2. Improve that factor and keep the scorecard green.
3. Ship the change.
4. Compare the corresponding privacy-safe real cohort.
5. Adjust factor weights only when repeated releases show a stable relationship. Never tune a deterministic test merely to reproduce a desired retention number.
