# Guy issue acceptance: #1284 and #1118

Verified on 2026-09-19 on Linux. Changes are committed locally, not deployed.

## Actual public workflow

Ran the shipped TypeScript SDK's `JcodeClient.launch`, `createSession`,
`setModel`, `run`, and `getHistory` against a freshly built private daemon and
harness bridge. The provider was the real `gpt-6-astra` route using existing
credentials. No provider, transport, tool, or SDK method was mocked.
The model executed a real bash command (`sleep 3; printf 'TOOL_OK\n'`).

```sh
JCODE_SDK_TEST_MODEL=gpt-6-astra \
  node sdk/typescript/test/live-text-framing.mjs ./target/selfdev/jcode
```

The test is opt-in because it requires credentials and consumes provider quota.
It creates an empty working directory, disables memory and autonomous wakes,
and closes and removes its private instance afterward. The shared daemon was
not restarted. Its binary remained `builds/versions/5afd4655e/jcode`.

## Final repeated acceptance

After the confidence audit described below, three consecutive independent
private-instance runs passed (background task `108179m7ns`, exit 0).
The rebuilt binary included the fresh-session fix committed as `df34e9273`.
It reported `v0.85.61-dev (5424d8785, dirty)`, because the shared checkout also
contained unrelated voice work. That voice work is not part of the fix branch.
Binary SHA-256:

```text
251299e99708c2ce4a8bf3c11d7be5508434bf46bdd8d68f7fc7c029bd39be51
```

| Observation | Run 1 | Run 2 | Run 3 |
| --- | --- | --- | --- |
| Fifteen fresh idle history calls, latency range | 2–8 ms | 2–6 ms | 1–7 ms |
| Three history calls on message acceptance | 1–2 ms | 1–2 ms | 1–3 ms |
| Five concurrent history calls during real bash execution | 5 ms each | 6 ms each | 6 ms each |
| Real turn duration | 8,631 ms | 6,772 ms | 7,741 ms |
| Final persisted history message count | 4 | 4 | 4 |

Every run asserted all of the following through public SDK methods/events:

- All five busy replies arrived before both `tool_done` and `turn_done`.
- The real bash result contained `TOOL_OK` and was not an error.
- Assistant messages were `CHECKING_HISTORY` (`text-1`) and `FRAMED_OK`
  (`text-2`), with explicit completion boundaries and distinct IDs.
- `turn.finalText` was exactly `FRAMED_OK`, excluding narration.
- Legacy `turn.text` retained both messages.
- Final persisted history contained `FRAMED_OK`.
- The private instance began with no sessions and its home was removed on close.

These observations validate the caller workflow on this provider and platform.
Three repeats do not prove every scheduling interleaving or every provider.

## Failures discovered only in public acceptance

### Partial transport frames

Initial concurrent SDK history calls timed out after 30 seconds. The bridge was
reading daemon frames with cancellation-unsafe `read_line` inside
`tokio::select!`, discarding partial frames when an API request won. The fix in
`b487e4222` retains byte buffers in both directions and enforces the size cap
across cancellations. Translator-only tests did not expose this failure.

### Fresh-session fallback during model prefetch

An initial post-transport-fix run passed: idle calls took 3–7 ms, five busy calls
5 ms each, and the real framed turn completed in 7,229 ms. Its binary SHA was
`7e57867e4704a9d7481b9bf923eb6ab14568a330555bac1431cf317d158d0781`.
However, independent repetitions on that identical binary disconnected during
idle history. The initial pass was therefore insufficient evidence of stability.

Private daemon logs identified a separate cause: successful history requests
start model-catalog prefetch, which can briefly own the agent mutex. A subsequent
request takes the persisted fallback, but a fresh session intentionally has no
snapshot before its first visible message. The missing file propagated an error
and closed the internal connection.

The follow-up accepts a typed `NotFound` only for a registered live session and
constructs an empty, unsaved snapshot without waiting for the agent. Missing
unregistered sessions and corrupt registered snapshots remain errors. The live
check was strengthened to fifteen fresh idle calls before each turn. The final
three runs above passed all 45 of these reads without disconnecting.

### Invalid model-generated tool arguments

One intermediate repeat reached inference and framing successfully but the model
sent null boolean arguments to bash, so the actual tool did not execute. The
acceptance assertion correctly failed rather than counting that as a busy-tool
success. The prompt now explicitly supplies valid boolean values. No assertion
was relaxed. This acceptance prompt correction does not claim to fix general
null-argument tool handling.

## Supporting regression coverage

- All 18 history tests passed (`763633zhz0`), including live-vs-persisted state,
  a deterministically queued competing turn, and releasing the agent lock before
  socket writes. The new fresh-session test holds the agent lock continuously
  across three concurrent reads in each of idle and processing states, checks
  metadata and no persisted file creation, and rejects missing unregistered and
  corrupt registered snapshots.
- 262 Rust API/bridge/provider/SDK tests passed for text framing, with two
  pre-existing ignored tests.
- 54 TypeScript SDK tests passed, covering framing, reasoning interleaving,
  corrections, unframed compatibility, final structured answers, and parity.
- Real daemon-to-bridge socket integration passed with a scripted provider and
  a real read tool. It covers consecutive assistant messages, reasoning within
  one message, and completed-message retry retraction. This is supporting
  deterministic evidence, not a substitute for real-provider acceptance.
- After the transport fix, all 129 bridge tests passed. Added cases cover three
  fragmented 1 MiB history replies with interleaved API ping, split UTF-8 across
  cancellation, and cumulative frame-size limits.
- Independent review found and verified fixes for previous-response rollback
  contamination and late recovered text suffixes missing completion markers.

No push, GitHub issue closure, shared-daemon reload, or production rollout was
performed as part of acceptance.

## Whole-result rerun and requirement traceability

After all implementation commits and the confidence audit, reran the complete
mapped checks instead of relying on results predating the follow-up fixes:

- `2258375prf`: protocol, bridge, OpenAI provider and Rust SDK suites passed
  (265 tests, two pre-existing ignored), then all 18 history tests and the root
  daemon/bridge socket E2E passed. This also compiled the TUI and CLI consumers.
- `2291904k6v`: rebuilt the shipped TypeScript distribution, passed all 54 SDK
  tests, then passed the real-provider public workflow (8,131 ms turn,
  idle history 2–7 ms, busy history 2–4 ms).
- `370249icam`: repeated the public workflow on the binary freshly rebuilt by
  the final integration suite, `v0.85.63-dev (465610a58)`, SHA-256
  `eb12aa164c80904ac302147ece867c46905c85780760b2636c29d17c6fbc973c`.
  The real turn passed in 6,730 ms. Fifteen idle histories took 2–8 ms,
  three message-accepted histories took 3 ms each, and five busy histories took
  2–4 ms. Two framed messages and four final persisted messages were returned.
  Busy replies preceded tool and turn completion. Actual tool output and
  private-instance cleanup passed. A bridge broken-pipe log occurred during
  client shutdown after the successful turn, not during history requests.

| Requirement / changed public output | Rerun check | Observed result |
| --- | --- | --- |
| #1284 available agent uses live history | `handle_get_history_uses_live_snapshot_when_agent_is_available` | Live transcript, rather than stale persisted snapshot, returned. |
| Retain mutex guard across selection and release before writes | `history_guard_survives_racing_turn_and_is_released_before_write` | Queued competing turn did not replace the selected snapshot. Socket backpressure did not retain the agent guard. |
| Busy history must not wait for the turn | `handle_get_history_falls_back_to_persisted_snapshot_when_agent_is_busy` plus real SDK busy probes | Snapshot returned with agent locked. Five actual replies arrived before real tool and turn completion. |
| Fresh unsaved sessions remain connected | `handle_get_history_busy_fresh_session_returns_empty_without_waiting` plus fifteen real idle probes | Concurrent locked reads returned empty history and metadata without saving. Missing unregistered and corrupt registered sessions still errored. Actual connection stayed open. |
| Fragmented concurrent replies retain all bytes | `concurrent_history_replies_survive_api_requests_between_fragments` and byte-reader tests | Three 1 MiB histories survived interleaved requests. Split UTF-8 survived cancellation. Accumulated frame limits remained enforced. |
| Additive API minor version, optional IDs and capability | Rust schema test, TS version/tag parity tests, actual launch assertion | New and legacy shapes accepted. Rust/TS versions and tags agreed. Actual hello advertised `text_framing`. |
| #1118 consecutive assistant boundaries | `output_item_completion_frames_messages_not_reasoning_or_text_chunks`, bridge tests, root socket E2E | Consecutive messages completed separately. Reasoning and text chunks did not invent boundaries. |
| Tool argument streaming must not prematurely end text | `text_framing_tools_and_turn_fallback_close_once_without_phantom_messages` | Tool-start did not close text. Execution/completion fallbacks closed once without phantom messages. |
| Corrections and retry retractions update completed messages | `text_retry_retracts_completed_and_live_messages_and_late_replacements_keep_ids`, Rust/TS collectors, root socket E2E | Discarded text removed, corrections retained IDs, retracted output excluded from final selection. Late recovered suffix completion assertions passed. |
| New-request retry must preserve earlier response | `new_request_retry_does_not_retract_previous_response` | Earlier response retained. Rollback scoped to current attempt. |
| SDK messages and final answer separate narration | Rust and TS collector tests plus real SDK | `CHECKING_HISTORY` and `FRAMED_OK` had separate IDs. Final text exactly `FRAMED_OK`. Aggregate text retained both. |
| Unframed compatibility, missing IDs, interleaved completions and reasoning-only turns | TS framing compatibility and Rust schema tests | Aggregate fallback preserved, ID-less boundaries collected, interleaved IDs correlated, no phantom reasoning-only messages. |
| Structured output validates final answer only | Rust `structured_output_uses_final_message_not_process_narration` and TS structured framing test | Final framed JSON validated despite preceding narration. |
| Internal event integrates with CLI/TUI | Root E2E rebuilt CLI/TUI and exercised daemon-to-bridge sockets | Exhaustive consumers compiled and socket workflow passed. No visual TUI behavior change is claimed or visually tested. |
| Persistence and private isolation | Actual final `getHistory`, initial `listSessions`, and `close` assertions | Final answer persisted, no existing sessions touched, private home removed. |

Parser/SDK edge cases use fixtures to force otherwise rare conditions. They
complement real-provider acceptance and are not live proof of every provider's
retry behavior. No mapped check failed in this final combined rerun. Only this
evidence document changed after these checks.
