# WebSocket prewarming validation

## Observed application-level benefit

On 2026-09-06 UTC, 20 first-message requests were run through real, isolated
Jcode daemons using the activated `a495fb059-dirty-40e6123ec268` binary.
Ten used `JCODE_OPENAI_PREWARM=0`, and ten used `JCODE_OPENAI_PREWARM=1`.
Both conditions used WebSocket v2. This isolates the benefit of **prewarming**,
not a v1-versus-v2 protocol comparison.

Each trial started a new daemon with a unique socket and `JCODE_RUNTIME_DIR`,
provider `openai-api`, model `gpt-5.6-sol`, and tool profile `none`. After the
subscription acknowledgment, both conditions received exactly 1.5 seconds of
simulated user think time. The warmed condition did not wait conditionally for
warmup readiness. The same prompt then requested exactly `OK` without tools.
Timing began when the client submitted the message and ended at its first
`text_delta` event. Pair order alternated cold/warm, then warm/cold.

| Observation | Prewarming disabled | Prewarming enabled |
| --- | ---: | ---: |
| Requests | 10 | 10 |
| Median time to first text | 1,471.87 ms | 1,079.82 ms |
| Mean time to first text | 1,613.05 ms | 1,082.46 ms |
| First-request socket reuse | 0/10 | 10/10 |
| Correct `OK` responses | 10/10 | 10/10 |

Prewarming was faster in all ten pairs. Median first-text latency was 392 ms
lower, a 26.6% reduction in this scenario. The paired mean difference was
531 ms, influenced by one slow cold request. No trial fell back to HTTPS.

### Raw time-to-first-text measurements

| Pair | Disabled (ms) | Enabled (ms) |
| --- | ---: | ---: |
| 1 | 1779.78 | 1097.47 |
| 2 | 1494.73 | 1027.85 |
| 3 | 1312.69 | 1106.45 |
| 4 | 1553.76 | 980.46 |
| 5 | 3301.52 | 1042.63 |
| 6 | 1201.89 | 1136.33 |
| 7 | 1449.00 | 927.14 |
| 8 | 1235.05 | 1229.63 |
| 9 | 1232.66 | 1062.16 |
| 10 | 1569.42 | 1214.49 |

### Limits

This is a small, local experiment with one model, account, prompt, and network
condition. It demonstrates a repeatable benefit when preparation can overlap
user think time, not a universal latency guarantee. Daemon startup and the equal
1.5-second think interval are excluded from the reported foreground latency.
Warmup performs additional network work. This experiment does not measure token
charges, immediate-input misses, long-idle expiry, tool-heavy requests, or a
population-level latency percentile. API-key routing was exercised live. The
configured OAuth credentials were unusable, so OAuth headers were checked only
in offline tests and no authentication was changed.

## Correctness and activation evidence

- 126 OpenAI runtime tests passed, including warmup cancellation, settings
  mismatch, preserved conversation/reasoning input, expiry, and credential changes
  before and during continuation preparation.
- Two idle-session tests passed, covering pre-input preparation, busy-session
  skipping, and release of the agent lock when preparation would yield.
- The active-provider delegation test passed.
- A live provider test passed cold generation, prewarmed generation, and subsequent
  continuation over the same response chain.
- An isolated full-daemon smoke test returned `OK` on its first message using
  `websocket/persistent-reuse` and an observed prewarm hit.
- The coordinated TUI build passed and the shared daemon was confirmed running
  the activated version before the repeated experiment.

See [transport behavior and controls](OPENAI_WEBSOCKET.md).
