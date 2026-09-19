# Guy issue acceptance: #1284 and #1118

Verified on 2026-09-19 on Linux. Changes are committed locally, not deployed.

## Actual public workflow

Ran the shipped TypeScript SDK's `JcodeClient.launch`, `createSession`,
`setModel`, `run`, and `getHistory` against a freshly built private daemon and
harness bridge. The provider was the real `gpt-6-astra` route using existing
credentials. No provider, transport, tool, or SDK method was mocked in this run.
The model executed a real bash command (`sleep 3; printf 'TOOL_OK\n'`).

```sh
JCODE_SDK_TEST_MODEL=gpt-6-astra \
  node sdk/typescript/test/live-text-framing.mjs ./target/selfdev/jcode
```

The test is opt-in because it requires credentials and consumes provider quota.
It creates an empty working directory, disables memory and autonomous wakes,
and closes and removes its private instance afterward. The shared daemon was
not restarted. Its binary remained `builds/versions/5afd4655e/jcode`.

Tested binary reported `v0.85.57-dev (0b8179f20, dirty)` and included the runtime
transport correction subsequently committed as `b487e4222`. SHA-256:

```text
7e57867e4704a9d7481b9bf923eb6ab14568a330555bac1431cf317d158d0781
```

## Observed behavior

| Requirement | Public-interface observation |
| --- | --- |
| Idle history remains available | Three concurrent SDK history calls returned in 3, 5, and 7 ms. |
| History does not wait for a turn | Three history requests immediately after `message_accepted` returned in 2, 2, and 3 ms. |
| Busy history returns before completion | Five concurrent requests during the real bash execution all returned in 5 ms, before `tool_done` and `turn_done`. |
| Assistant messages have explicit boundaries | SDK returned `CHECKING_HISTORY` as `text-1` and `FRAMED_OK` as `text-2`, each terminated by `text_done`. |
| Final answer excludes tool narration | `turn.finalText` was exactly `FRAMED_OK`. |
| Existing aggregate text remains usable | `turn.text` contained both narration and final answer. |
| Persistence still works | Final history had four messages and contained `FRAMED_OK`. |
| Isolation and cleanup | Instance began with no sessions and its home was absent after `close()`. |

The real turn completed in 7,229 ms. The acceptance process exited successfully.
These observations demonstrate the actual caller workflow, not a statistical
claim about every possible scheduling interleaving or every provider.

## Problem discovered only in public acceptance

The first real SDK run failed with `no reply to get_history within 30000ms`
while making concurrent idle history requests. A second run returned one reply
in 2 ms and then disconnected. Neither reached inference.

The bridge was reading daemon frames with cancellation-unsafe `read_line`
inside `tokio::select!`, discarding partial frames when an API request won.
The follow-up uses retained byte buffers for both directions and enforces the
size cap across cancellations. Rebuilding with this change converted the same
public SDK workflow from timeout/disconnection to the successful measurements
above. This was not detectable from translator-only unit tests.

## Supporting regression coverage

- 17 history tests passed, including live-vs-persisted state, a deterministically
  queued competing turn, and release of the agent lock before socket writes.
- 262 Rust API/bridge/provider/SDK tests passed for text framing, with two
  pre-existing ignored tests.
- 54 TypeScript SDK tests passed, covering framing, reasoning interleaving,
  corrections, unframed compatibility, final structured answers, and parity.
- Real daemon-to-bridge socket integration passed with a scripted provider and
  a real read tool. It covers consecutive assistant messages, reasoning within
  one message, and completed-message retry retraction. This is supporting
  deterministic evidence, not a substitute for the real-provider run above.
- After the live-discovered transport fix, all 129 bridge tests passed. Added
  cases cover three fragmented 1 MiB history replies with interleaved API ping,
  split UTF-8 across cancellation, and cumulative frame-size limits.
- Independent review found and verified fixes for previous-response rollback
  contamination and late recovered text suffixes missing completion markers.

No push, GitHub issue closure, shared-daemon reload, or production rollout was
performed as part of acceptance.
