# Browser handoff paired benchmark

`benchmark_browser_handoff.py` is a Python 3 standard-library-only benchmark.
It does not claim handoff is always faster. It measures actual fresh-session
behavior on a synthetic task suite against direct browser tools, using the same
requested parent model/provider. The default explicit Jev experiment is separate
from the optional natural-default trigger experiment.

## Coordinator preparation and execution

1. Build the TUI binary with the desired browser changes. Use the exact built
   binary path, not the launcher symlink. The harness records its resolved path
   and SHA-256 and explicitly launches that binary's isolated daemon.
2. Check browser readiness and set up only if necessary. Create one disposable
   tab showing a loopback fixture with title `Jcode isolated browser fixture`
   or `Jev hybrid verified`, and record its numeric ID. The harness refuses
   non-loopback and non-fixture initial tabs and never opens tabs/windows. Do not use a personal/work tab. Reserve the tab
   exclusively for this serial benchmark. The browser must be on this machine
   and able to reach `127.0.0.1`.
3. Prepare an isolated Jcode home outside the repository with the required
   auth/config and `browser/browser` bridge. The runner does not copy credentials
   or modify the normal home. Export the existing `BROWSER_SESSION` matching the
   dedicated tab. Keep credentials private.
4. Run from the repository root, substituting the actual model, isolated home,
   and tab ID:

```bash
python3 scripts/benchmark_browser_handoff.py --self-test
python3 scripts/benchmark_browser_handoff.py \
  --binary "$PWD/target/selfdev/jcode" \
  --model 'openai-api:gpt-6-astra' \
  --jcode-home "$JCODE_SCRATCH_DIR/prepared-browser-benchmark-home" \
  --tab-id 123 \
  --trials 3 --timeout 240 \
  --output "$JCODE_SCRATCH_DIR/browser-handoff-$(date +%s)"
```

Use an available model/route, not necessarily the example above. If needed,
pass `--provider` as well. Defaults are `--arm jev --phase all` and all three
tasks. `--trials 3` means three pairs **per task per phase**, or **36 sessions**.
Use `--phase discovery` first (18 sessions), freeze implementation and prompts,
then `--phase heldout` into a new output directory (18 sessions). `--phase all`
runs discovery before held-out but offers no pause for tuning. Held-out variants
are fixed regression holdouts, not secret/unseen tasks or a generalization proof.
Use `--tasks navigation` for the old six-session scale with `--phase discovery`.
Fewer than three pairs are permitted for smoke checks but cannot meet the target.
The output directory must not already exist. Each trial starts a new daemon on
its own private `--socket`, runs a fresh `jcode run --ndjson` in a new workspace,
and terminates only that daemon/process group. Shared Jcode daemons are not
restarted or repointed. The caller-prepared isolated home is used without copying credentials into
reports. Browser-session and provider environment settings are inherited. Startup probing and daemon shutdown are excluded from elapsed time.
Browser setup is a coordinator prerequisite, not a measured step. The harness
enables `JCODE_DEBUG_CONTROL=1` only in its isolated daemon/client environment
for `debug --socket <trial-socket> server:info` readiness checks. Startup failures
include the last probe response and the exact daemon log path.

`--arm jev` explicitly asks the parent to delegate the whole task and supplied
text to Jev. `--arm normal` instead measures natural-default routing and never
mentions handoff in that arm's prompt. Do not pool these experiments. The direct
arm prohibits handoff in its prompt and sets `JCODE_BROWSER_HANDOFF_DISABLED=1`
in **both daemon and client** environments. Other arms set it to `0`. The runtime
guard activates only for the exact value `1`: it removes handoff and its task-only
fields from the schema, replaces the handoff-default guidance with direct-action
guidance, and rejects explicit handoff execution before provider setup. Unset or
`0` retains normal behavior. The tested binary must include this guard.
The harness records the guard request, not proof that an old binary implements
it. Direct handoff calls are retained as protocol violations and failures.
Both arms use the browser tool only
and target the same explicit tab. A trial-unique localhost URL resets the task.
Trial order alternates Jev/direct then direct/Jev (or normal/direct) to reduce order bias.
No retries are silently discarded. Sessions, fixture tokens, and receipts are
fresh. Global provider/browser caches and the prepared isolated home are **not**
cleared between trials, so this is a fresh-session comparison, not a cold-cache experiment.

## Tasks and acceptance target

- Navigation: four nested documentation links to a fresh receipt. Held-out
  changes the target guide and examples.
- Search/filter: enter a catalog term, choose a category, submit, then navigate
  item details and specifications to a receipt. Held-out changes term/category.
- Form: navigate to a synthetic sample request, fill supplied name, invalid-domain
  email and notes, choose pickup, check confirmation, submit and view receipt.
  Held-out changes all supplied text. Data remains in the loopback fixture only.

Both arms receive the same task and data in a pair, with unique URLs/receipts.
The server validates ordered required route visits plus exact search/form fields.
Wrong fields cannot reveal a receipt. DOM validation separately verifies the
final page. Navigation cannot pass by merely guessing the final receipt URL.

Design target: at least 3 pairs per task in discovery, then 3 per held-out task,
median paired direct/Jev end-to-end ratio >=2 with no lower Jev success count.
`target_met` additionally requires >=3 correct eligible pairs and >=2 median
all-attempt ratio in every represented phase/task group. A discovery-only report
cannot certify held-out acceptance. `full_suite_target_met` requires all six
phase/task groups to be present and pass. Natural-default runs do not satisfy the
explicit-Jev acceptance flag. Small-sample medians are descriptive, not a
statistical guarantee. Preserve failed runs and report both phases separately.

## Evidence and interpretation

- `metrics.ndjson` and stdout: one JSON record per trial plus a summary.
- `summary.json`: trigger rate, valid success counts, eligible paired elapsed
  ratios and median, per-phase/task summaries, all-attempt raw latencies and
  paired ratios, failures, timeouts and protocol violations. Ratio `direct /
  Jev > 1` favors handoff. Success-only ratios are explicitly separate from
  all-attempt ratios. Timeouts are censored observations, not successful fast
  completions. Infrastructure failures remain in the denominator and include
  startup time with an explicit timing-scope label.
- `metadata.json`: binary hash, model/provider, and timing definition.
- Per-trial directories: original prompt, raw NDJSON transcript, stderr,
  structured result, and isolated workspace. Separate daemon logs are retained.
- Tool traces reconstruct streamed JSON inputs from `tool_start`, `tool_input`,
  `tool_exec`, and `tool_done`. Only executed browser actions count. Assistant
  prose saying “handoff” does not count. Unknown/missing actions invalidate a
  trial for speed comparison. Tool errors remain visible in the trace. Handoff result status and the number
  of `action_trace` steps marked `executed` are captured per call and per trial. Actual
  handoff `decision_provider` receipts must equal `jcode` by default. For explicit
  BYOK comparisons use `--expected-handoff-provider openrouter`. Missing or
  mismatched provider metadata invalidates the trial, rather than silently
  reporting subscription success.
- Correctness is independent of the agent's claimed success: the fixture must
  serve the fresh receipt page and receive a page-owned JavaScript beacon after
  its DOM exists. A pagehide beacon clears the visible flag. After the timed
  client exits, the runner makes a separate read-only bridge `evaluate` call in
  the designated tab to verify the exact final URL, heading and fresh receipt.
  Only booleans are returned, not unrelated tab contents. The agent must also
  return the fresh receipt. HTTP request history is retained. These checks
  validate DOM state, not pixels or an adversarial anti-cheating guarantee.
  Inspect raw traces for suspicious shortcuts.
- A successful direct arm that nevertheless called handoff is protocol-invalid.
  Other executed tools likewise invalidate the browser-only task. Only correct,
  compliant pairs where the comparison arm has an error-free `done` handoff or a
  handoff with at least one executed step contribute to speed ratios. A failed
  or zero-progress handback followed by direct success is not speed-eligible. Natural-default arms that chose direct actions still count in the trigger rate.
- Parent tool count counts actual executed tool IDs, not textual mentions.
  `usage_tokens` preserves the NDJSON `done.usage` snapshot with a scope label:
  this is **last reported usage, not a task-total claim**. Raw `tokens` events
  remain in the trace, without unsafe summation of potentially cumulative data.
  `model_call_count` and `actual_cost_usd` are taken only from corresponding
  explicit `done` fields. They are `null` (unavailable) when telemetry does not
  provide them. Counts are not inferred from tokens/tool calls and subscription
  usage is not converted to invented dollars. These parent fields do not imply
  inclusion of Jev's internal model calls or usage.
- Handoff output timing instrumentation is extracted recursively with original
  paths and units intact under each call's `timing_instrumentation`, including
  timing objects and elapsed/duration/latency fields within action traces.
  Missing instrumentation remains absent rather than estimated.
- Full elapsed time runs from client process launch until exit, including model
  reasoning and final response. `seconds_to_confirmation` additionally records
  the fixture beacon time. Neither isolates browser execution alone.

The fixture contains only invented data and performs no external writes. It
binds only to loopback and serves unguessable trial paths. Output is created with
mode 0700, but raw model/daemon logs may still contain environment-specific
information. Review logs before sharing. The harness intentionally does not
archive environment variables or credential files. Live runs consume model and
handoff-provider credits and manipulate the designated tab.

`--self-test` is backward compatible and exercises trace reconstruction,
assistant-prose false positives, provider receipts, effective handoff accounting,
telemetry availability, recursive timing extraction, strict arm environment,
failure/timeout retention, target minimums, all six task/phase fixtures, wrong
search/form values, route ordering, premature receipt rejection, pagehide reset
and DOM beacon requirements through local HTTP requests. It launches neither
Jcode nor a browser. Live correctness and speed remain the coordinator's
responsibility after building and preparing the tab. No live speedup is implied
by passing these self-tests.
