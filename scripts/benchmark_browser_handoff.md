# Browser handoff paired benchmark

`benchmark_browser_handoff.py` is a Python 3 standard-library-only benchmark.
It does not claim handoff is always faster. It measures actual fresh-session
behavior under an ordinary documentation-navigation prompt against the same prompt with
handoff explicitly prohibited, using the same requested model/provider.

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
pass `--provider` as well. `--trials 3` runs **six sessions** (three pairs).
The output directory must not already exist. Each trial starts a new daemon on
its own private `--socket`, runs a fresh `jcode run --ndjson` in a new workspace,
and terminates only that daemon/process group. Shared Jcode daemons are not
restarted or repointed. The caller-prepared isolated home is used without copying credentials into
reports. Browser-session and provider environment settings are inherited. Startup probing and daemon shutdown are excluded from elapsed time.
Browser setup is a coordinator prerequisite, not a measured step.

The normal prompt never mentions handoff. Both arms use the browser tool only
and target the same explicit tab. A trial-unique localhost URL resets the task.
Trial order alternates normal/direct then direct/normal to reduce order bias.
No retries are silently discarded. Sessions, fixture tokens, and receipts are
fresh. Global provider/browser caches and the prepared isolated home are **not**
cleared between trials, so this is a fresh-session comparison, not a cold-cache experiment.

## Evidence and interpretation

- `metrics.ndjson` and stdout: one JSON record per trial plus a summary.
- `summary.json`: trigger rate, valid success counts, eligible paired elapsed
  ratios, and median. Ratio `direct / normal > 1` favors handoff.
- `metadata.json`: binary hash, model/provider, and timing definition.
- Per-trial directories: original prompt, raw NDJSON transcript, stderr,
  structured result, and isolated workspace. Separate daemon logs are retained.
- Tool traces reconstruct streamed JSON inputs from `tool_start`, `tool_input`,
  `tool_exec`, and `tool_done`. Only executed browser actions count. Assistant
  prose saying “handoff” does not count. Unknown/missing actions invalidate a
  trial for speed comparison. Tool errors remain visible in the trace. Actual
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
  compliant pairs where the normal arm actually used handoff contribute to speed
  ratios. Normal arms that chose direct actions still count in the trigger rate.
- Full elapsed time runs from client process launch until exit, including model
  reasoning and final response. `seconds_to_confirmation` additionally records
  the fixture beacon time. Neither isolates browser execution alone.

The fixture contains only invented data and performs no external writes. It
binds only to loopback and serves unguessable trial paths. Output is created with
mode 0700, but raw model/daemon logs may still contain environment-specific
information. Review logs before sharing. The harness intentionally does not
archive environment variables or credential files. Live runs consume model and
handoff-provider credits and manipulate the designated tab.

`--self-test` exercises trace reconstruction, assistant-prose false positives,
empty-summary handling, fixture navigation pages,
provider-receipt extraction, pagehide reset, and the DOM beacon requirement through local HTTP requests. It launches neither
Jcode nor a browser. Live browser correctness and speed must be validated by the
coordinator after building and preparing the tab.
