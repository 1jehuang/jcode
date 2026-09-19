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
