# Fast browser handoff

The shared app-core browser tool supports `action: "handoff"`. The normal agent
supplies the goal, an explicit tab, optional exact actions, and any exact typing
values. A bounded controller uses OpenRouter's typed Decisions API with
`typesafe/jev-1.13` to choose an offered action ID, `done`, `hand_back`,
`script_needed`, or `text_needed`. Jev is the fast general-purpose classifier.
The main LLM supplies generated code or text only when asked, then hands the
next stretch back to Jev. Jev never generates executable browser arguments.

This is the preferred multi-step browser path in the tool description. Direct
browser actions remain available. The implementation lives in the shared
runtime, so Desktop and the TUI use the same controller.

## Setup

Check `browser` with `action: "status"` first and run setup only if not ready.
Connect OpenRouter using `jcode login openrouter`. The controller reads
`OPENROUTER_API_KEY` or the owner-only `~/.config/jcode/openrouter.env` file,
never a different OpenAI-compatible provider's credential.

Jev uses `POST https://openrouter.ai/api/alpha/decisions`, not chat completions.
Its requests use OpenRouter credits and honor the key's usage cap. Jcode does
not buy credits or switch your main coding model.

## Interface and control boundary

```json
{
  "action": "handoff",
  "tab_id": 301,
  "window_id": 710,
  "frame_id": 0,
  "goal": "Open Documentation, then Browser controls. Finish when the verification message is visible.",
  "max_steps": 8,
  "confidence_threshold": 0.8,
  "text_values": [],
  "candidates": []
}
```

- `tab_id` and a nonempty `goal` are required. Frame 0 is the default.
  `window_id`, when supplied, is checked against the selected tab.
- `max_steps` defaults to 12 and must be 1 through 30. The controller has a
  180-second wall-clock budget. `timeout_ms` controls individual operations,
  defaults to 20,000, and is clamped to 1 through 60,000 milliseconds and the
  remaining total budget.
- `confidence_threshold` defaults to 0.8. The transport validates the typed
  choice, offered IDs, probability distribution, and confidence. It uses the
  lower of the selected probability and reported confidence.
- `text_values` are exact, non-sensitive strings from the parent. Do not pass
  passwords, OTPs, credentials, or payment details. DOM form values are omitted
  from observations. Sensitive pages and recognized credentials cause handback.
- `candidates` contains `{ "label": "...", "input": { "action": "...", ... } }`
  entries. These are trusted parent-authorized payloads, not page instructions.
  They are one-shot and bound to the initial page URL. A URL change requires
  renewed parent authorization. Broad actions such as evaluation or upload must
  be explicitly supplied by the parent. Recursive handoff, setup, new tabs, and
  raw commands that cannot be scoped are rejected.
- Content actions target one tab and frame. Whole-tab actions cannot honor a
  nonzero frame target and are rejected there. Frame targeting is not a sandbox
  for arbitrary parent-provided JavaScript.

Every decision receives a fresh bounded DOM observation. Before execution or
accepting `done`, the controller observes again and hands back if the observed
DOM changed. Observations include node identities and scroll position. The
result contains `status`, `reason`, `requested_help`, `action_trace`,
`final_observation`, and `model`. A help choice returns `status: "hand_back"`
with `requested_help: "script"` or `"text"`. The parent reads the goal and page
observation, supplies a trusted exact `eval`/other action in `candidates` or the
needed `text_values`, then invokes handoff again. Other handbacks use
`requested_help: "uncertain"`; successful completion has no requested help.
On resume, explicitly state in the goal that the supplied action is ready. For
example: "The parent-supplied title-setting action is ready. Use it to set
document.title to 'Jev hybrid verified', then finish after verifying the fresh
page title." Do not repeat a request for a missing script once it is supplied.

Candidate results and screenshots return to the parent. Jev is text-only and
does not receive screenshot pixels or arbitrary action-result payloads. Known
credential patterns are redacted, but this is not a complete secret detector.
Do not delegate confidential page content you do not want sent to OpenRouter
and TypeSafe. A screenshot returned to the parent cannot be text-redacted.

An uncertain in-flight action must not be blindly retried: cancellation
or a timeout cannot undo an action already delivered to Firefox.

Page content and action labels derived from it remain untrusted. Navigation
heuristics and sensitive-word filters reduce accidental actions, but cannot
prove that an arbitrary website's link, input handler, or button is harmless.
Use a dedicated tab and narrowly authorized actions for consequential work.
A pre-action observation is not an atomic DOM transaction with execution.
The parent must inspect handback evidence rather than treating high model
confidence as authorization or a security guarantee.

## Acceptance against a newly built CLI

Building alone does not update the shared daemon. Do not send a new-feature
acceptance command to the shared server and assume it measures the new binary.
A custom socket alone also does not isolate persistent swarm recovery: use an
isolated runtime directory and home. Never reuse a user session ID.

The caller must prepare a disposable local fixture tab and an existing dedicated
`BROWSER_SESSION`. Without that environment variable, the bridge can create an
agent browser session/window automatically. The commands below do not create
browser tabs. They assume `OPENROUTER_API_KEY` is already available in the
process environment, without putting its value in shell history or logs.

```bash
cargo build --profile selfdev
BIN="$PWD/target/selfdev/jcode"
REAL_JCODE_HOME="${JCODE_HOME:-$HOME/.jcode}"
export JCODE_HOME="$(mktemp -d "$JCODE_SCRATCH_DIR/browser-fast-home.XXXXXX")"
export JCODE_RUNTIME_DIR="$(mktemp -d "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/jbf.XXXXXX")"
SOCK="$JCODE_RUNTIME_DIR/jcode-browser-fast.sock"
cp -a "$REAL_JCODE_HOME/browser" "$JCODE_HOME/browser"
: "${OPENROUTER_API_KEY:?Provide OpenRouter credentials through the process environment}"
: "${BROWSER_SESSION:?Use the existing dedicated fixture browser session}"
: "${JCODE_BROWSER_HANDOFF_TEST_TAB_ID:?Use a disposable local fixture tab}"

# Enable debug control only for this disposable acceptance daemon.
JCODE_DEBUG_CONTROL=1 "$BIN" --no-update --provider openrouter --socket "$SOCK" serve \
  --temporary-server --owner-pid "$$" --temp-idle-timeout-secs 300 \
  >"$JCODE_HOME/acceptance-server.log" 2>&1 &
SERVER_PID=$!
# Wait for this private debug listener, never fall back to the shared socket.
for attempt in $(seq 1 100); do
  test -S "${SOCK%.sock}-debug.sock" && break
  sleep 0.1
done
test -S "${SOCK%.sock}-debug.sock" || exit 1
readlink -f "/proc/$SERVER_PID/exe"
"$BIN" debug --socket "$SOCK" server:info
SID=$("$BIN" debug --socket "$SOCK" create_session "$PWD" |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])')

PAYLOAD=$(python3 -c 'import json,os; print(json.dumps({
  "action":"handoff", "tab_id":int(os.environ["JCODE_BROWSER_HANDOFF_TEST_TAB_ID"]),
  "frame_id":0, "max_steps":8,
  "goal":"Open Documentation, then Browser controls. Finish only when Fast browser integration verified is visible. Stay on the local fixture website."
}))')
"$BIN" debug --socket "$SOCK" --session "$SID" tool "browser $PAYLOAD"
```

The debug CLI has two positional arguments: `COMMAND` and `ARG`. It joins them
as `COMMAND:ARG`, so the exact direct-tool syntax is
`debug --socket "$SOCK" --session "$SID" tool 'browser {"action":"..."}'`.
`debug tool browser JSON` is not valid syntax. The temporary server is owned by
this shell and can expire when its owner exits. Do not run shared-server stop,
reload, or promotion commands to clean up an acceptance daemon.

## Opt-in live tests

Run `python3 scripts/browser_handoff_fixture.py` to serve the disposable pages
on an ephemeral loopback port. It never opens a browser or reads credentials.
Open its printed URL in a dedicated test tab. Reset that tab to `/` before each
navigation test. Use `/blocked` for the authentication handback test.

The reproducible runner owns a local HTTP fixture for the entire test sequence,
resets its designated disposable tab before each case, and clears that tab on
exit. It refuses a non-fixture tab. Prepare an `about:blank` disposable tab or
reuse a prior local Jcode fixture, and use an existing browser session:

```bash
BROWSER_SESSION=<existing-session-name> \
  python3 scripts/test_browser_handoff_live.py --tab-id <disposable-tab-id>
```

This makes small paid Jev calls and tests the transport, two-link navigation,
`script_needed` followed by a parent-supplied exact script and Jev resumption,
and an authentication prompt that hands back without actions. The 0.8 execution
threshold is unchanged. The script never opens or focuses a browser window.

`browser_fast_live_tests.rs` contains ignored tests that call the real
`BrowserTool::execute`, not a copied controller or mock provider. All require
an existing `BROWSER_SESSION` and refuse a non-loopback initial fixture URL.
They do not create, select, focus, or close a tab/window themselves.

```bash
# Start page -> Documentation -> Browser controls -> visible verification text.
JCODE_BROWSER_HANDOFF_TEST_TAB_ID=<dedicated-tab-id> \
  cargo test -p jcode-app-core live_browser_handoff_completes_local_navigation \
  -- --ignored --nocapture

# A separate dedicated local page with a visible OTP/password control.
JCODE_BROWSER_HANDOFF_TEST_BLOCKED_TAB_ID=<dedicated-blocked-tab-id> \
  cargo test -p jcode-app-core live_browser_handoff_sensitive_fixture_hands_back_without_actions \
  -- --ignored --nocapture
```

The navigation test requires `done`, at least two executed clicks, a bounded
trace, and a final observation containing `Fast browser integration verified`.
The sensitive-page test requires `hand_back`, a sensitive observation, and no
actions. These use the currently compiled test binary. The separate isolated
CLI procedure verifies the CLI/daemon boundary too. Passing mock tests alone
is not evidence of either live acceptance workflow.

## Safe shared-server activation

Inspect only while other work is active:

```bash
SHARED="/run/user/$(id -u)/jcode.sock"
jcode debug --socket "$SHARED" sessions
jcode debug --socket "$SHARED" clients:map
jcode debug --socket "$SHARED" background:tasks
jcode debug --socket "$SHARED" jobs
jcode debug --socket "$SHARED" server:info
```

`sessions` exposes `is_processing` and `status`. Defer activation while any
session is processing/running or other work cannot safely checkpoint. An idle
snapshot has a race with new work: coordinate a quiescent window with operators.
The reload implementation fires graceful shutdown signals at running sessions,
so session preservation is not the same as uninterrupted generation.

Only after validation, explicit activation authorization, installation of the
immutable tested version, and an agreed idle window:

```bash
jcode server promote <installed-version> --json
jcode server reload --json
```

Promotion selects the daemon binary but does not replace the running process.
Reload applies the selection. Recheck `server:info` and session availability
afterward. Do not use `server stop --force`, kill signals, or an automatic
self-dev reload while user work is running.
