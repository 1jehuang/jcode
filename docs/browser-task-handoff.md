# Jev browser task handoff

`browser(action="handoff")` delegates a whole task in one explicit tab and frame.
Jcode owns the task state and execution loop. Jev selects the next action from the
current trusted action catalog using the typed Decisions API. It does not generate
JavaScript or run a separate chat-completion agent.

Each cycle sends:

- The original goal and optional caller-supplied `context` (background and completion criteria).
- A fresh, compact observation of the current document and visible controls.
- Previous actions, their results, and evidence from pages visited earlier.
- Available browser actions and the remaining action budget.

Jcode executes the selected action, observes its result, appends that evidence, and
consults Jev again. Navigation is an intermediate step, not an implicit handback.
The default budget is 40 actions, with a maximum of 100 and a ten-minute task deadline.
Results use a rolling retention budget, and the transport compacts older history to
fit its final serialized request limit. The parent receives the trace and final
observation, not a free-form research report from Jev.

## Recovery and boundaries

- Main-content controls take precedence over repetitive header navigation. Observations
  include compact selectors, document/node identities, and nested scroll containers.
- Known safe search fields can type **and submit** caller-supplied text. Ordinary
  field filling remains separate. Form targets and submitter overrides must remain
  in the scoped browsing context.
- A changed target causes bounded replanning, not execution of a stale selector.
  Unrelated page changes need not invalidate an unchanged automatic action target.
- Navigation disconnect recovery retries observations, never the mutation itself.
  An uncertain consequential action is returned to the parent rather than replayed.
- Automatic scrolling and waiting may gather more evidence at low confidence.
  An uncertain interaction first narrows the menu to those observation actions,
  without executing the interaction, then reconsiders after gathering evidence.
  Interactions, exact caller actions and completion still require the configured
  confidence threshold. Exploration remains subject to scope, stall and budget limits.
- Exact caller actions are one-shot and retired on document or URL changes.
- Passwords, verification flows, credentials, missing text/scripts, persistent stalls,
  insufficient confidence, and exhausted budgets can still cause a handback.
  Page content and action results are evidence, never authorization.

Jev still cannot invent scripts or arbitrary typing values. If the catalog cannot
express the next step, `requested_help=script` or `text` asks the parent for the
missing capability. This is a genuine capability boundary, not normal per-action
orchestration.

## Verification

Run the browser regression suite through `selfdev test`:

```sh
cargo test -p jcode-app-core --lib browser
```

For live acceptance, check browser readiness, create a dedicated disposable
`about:blank` tab, and use its existing browser session:

```sh
BROWSER_SESSION=<existing-session> python3 scripts/test_browser_handoff_live.py --tab-id <disposable-tab>
```

The runner refuses non-fixture user tabs. It uses loopback pages and tests navigation,
search followed by multiple links and nested scrolling in a **single handoff**, script
help/resumption, and sensitive-flow handback. Live tests make small paid Jev requests.
They exercise the freshly compiled tool directly, rather than silently measuring an
older shared daemon binary.
