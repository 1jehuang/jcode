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

## Requirement-to-evidence ledger

This ledger maps the implemented scope, rather than treating the aggregate test
count as proof of every behavior. **Passed test** means the named test completed
successfully. **Live** means an observed provider or daemon result. **Source
check** means an explicit source-contract assertion or review, not a runtime
exercise. A complete mapping does not imply complete live-provider coverage.

| Requirement or changed public output | Concrete check | Observed result |
| --- | --- | --- |
| Explicit v2 negotiation on new connections | `v2_handshake_preserves_api_key_and_oauth_authentication`; loopback handshake in `websocket_v2_prewarm_is_adopted_by_complete_without_losing_request_state` | Passed. Header is `OpenAI-Beta: responses_websockets=2026-02-06`; authorization and OAuth account/originator headers are preserved. |
| Keep the existing Responses endpoint, not `/v2/responses` | Loopback handshake path assertion; `openai_catalog_and_chat_endpoints_agree_on_credential_shape` | Passed. Local API request used `/v1/responses`; credential-shape routing tests passed. |
| Prepare static settings without conversation input or model output | `warmup_only_prepares_settings_and_keeps_original_request_unchanged`; loopback adoption test | Passed. Warm request has `generate:false`, empty `input`, `store:false`; history and original request are unchanged. Only the subsequent generated response reaches the foreground stream. |
| Preserve all request settings and reject stale prefixes | `warmup_compatibility_compares_all_settings_but_not_conversation_input`; `ready_prewarm_with_different_settings_is_invalidated` | Passed. Every tested setting participates in equality; changed instructions reject a ready socket. |
| Preserve initial conversation and encrypted reasoning on warm continuation | `websocket_v2_prewarm_is_adopted_by_complete_without_losing_request_state` | Passed. First and current user turns plus encrypted reasoning are present, `previous_response_id` is the warm response ID, and `generate:false` does not leak into generation. |
| Continue the resulting response chain | `live_openai_v2_prewarm_and_continuation`, explicitly run with `--ignored` | Passed live. Cold, warmed, and subsequent continued responses returned `OK`; the warmed chain reached two generated messages. |
| Never await unfinished network warmup in the foreground | `foreground_cancels_matching_unfinished_warmup_without_waiting`; `unfinished_or_incompatible_prewarm_is_cancelled_without_foreground_wait` | Passed. Pending work is aborted, no state adopted, foreground take returns within the test's 100 ms bound, and the loopback socket closes. |
| Warm before input without blocking an active session | `idle_prewarm_starts_before_user_input_and_skips_busy_sessions`; daemon first-message smoke | Passed. Busy agent skipped; idle hook invoked without a user message; live first message used `websocket/persistent-reuse`. |
| Do not hold the agent lock across pending preparation | `idle_prewarm_never_holds_agent_lock_across_pending_preparation`; source check S9 | Passed. Pending hook cancelled and the agent lock immediately available. Subscription `Done` precedes the single nonblocking poll. |
| Do not pin MCP discovery early; include canary tools | Source check S10 on `Agent::prewarm_provider(&self)` | Passed source check. Canary tools registered; unlocked snapshot built without assigning `locked_tools` or consuming the late-MCP flag. This specific canary path was not separately exercised live. |
| Try preparation before local context work in both turn paths | Source checks S12 on `turn_loops.rs` and `turn_streaming_mpsc.rs` | Passed source checks. Each `prewarm` call precedes `messages_for_provider`; both files compiled in the successful TUI build. |
| Delegate only to the active provider; preserve other providers' behavior | `prewarm_delegates_only_to_active_provider_without_completing`; default trait-method review and TUI build | Passed. Only active mock notified, no completion called; other implementations inherit the default no-op. |
| Existing conversation socket takes precedence | Source check S4 in `OpenAIProvider::prewarm`; live continued-response test | Passed source check for the existing/busy socket early return; live continuation retained its existing response chain. |
| Forks do not share speculative sockets; resets clear speculation | Source checks S5 and S6; `test_set_model_clears_persistent_ws_state`; `test_switching_to_https_clears_persistent_ws_state` | Passed. Fork constructs a new slot; both reset helpers clear it. Existing model/transport state-reset regressions passed. Fork warm-slot independence itself is source-checked, not a separate live fork experiment. |
| Never send through a socket authenticated to stale credentials | `persistent_ws_rejects_identity_changed_by_another_fork`; `persistent_ws_rechecks_identity_after_presend_backpressure`; source check S11 | Passed. Already-stale and changed-during-preparation identities rejected before generation; credential read guard held through frame flush. |
| Speculative cancellation must not rotate OAuth refresh tokens | `expired_credentials_are_not_refreshed_by_speculation` | Passed. Expired credentials rejected immediately, no network refresh performed, access token unchanged. |
| Bound warmup and expire unused state | `expired_ready_warmup_is_closed_instead_of_adopted`; source check S1 | Passed expiry-rejection/socket-close test. Source assertions confirmed 5-second timeout and 30-second cleanup timer. Automatic wall-clock cleanup at exactly 30 seconds was not separately timed. |
| Reject failed or malformed preparation without poisoning normal generation | `rejected_warmup_is_not_adopted`; `warm_socket` and failure-branch review | Passed failed-status rejection test. Source checks reject unexpected events/output or mismatched IDs and keep warmup failure outside foreground errors/cooldown updates. Malformed-event variants were reviewed, not all injected live. |
| Preserve normal WebSocket recovery and HTTPS fallback | `test_record_websocket_fallback_sets_cooldown_for_auto_default_models`; `test_record_websocket_fallback_tracks_streak_and_cooldown`; cancellation/reuse regressions | Passed existing recovery-policy tests. The live latency experiment observed no fallback and therefore does not independently prove a forced live HTTPS failover. |
| Public `JCODE_OPENAI_PREWARM` control | Ten live disabled controls versus ten enabled trials; source check S2 | `0` produced 0/10 first-request reuses; `1` produced 10/10. Source assertion confirmed trimmed, case-insensitive `false` and `off` aliases. Alias spellings were not separately exercised live. |
| HTTPS and browser-backed providers skip preparation | Source check S3; HTTPS state-reset test | Passed early-return source assertions and reset regression. No additional live browser-provider test was performed. |
| Public protocol diagnostic | Source check S7; runtime diagnostic-log scan | `websocket_protocol=v2` confirmed in formatter and observed in runtime logs. |
| Public lifecycle diagnostics | Source check S8; runtime log scan restricted to structured lifecycle lines | All four observed: `ws_prewarm_ready`, `ws_prewarm_hit`, `ws_prewarm_miss`, `ws_prewarm_unavailable`. Scan found 21, 11, 2, and 2 occurrences respectively at verification time. |
| Do not expose credentials or warmup inputs in new diagnostics | Review of all four warmup logging call sites and stored-identity usage | New fields contain only model, elapsed/age, compatibility, and protocol. The credential identity is used for comparison, not logged. This is a scoped source review, not a whole-program secret-leak audit. |
| Public connection label for a warm hit | Actual daemon smoke and repeated enabled trials | Observed `websocket/persistent-reuse` on the first generated request, with correct `OK` output. |
| Demonstrable benefit rather than header-only claims | Alternating enabled/disabled real-daemon experiment above | Median foreground first text improved by 392 ms (26.6%); 10/10 pairs faster; all 20 responses correct. Both arms used v2, so this attributes the observed benefit to preparation. |
| Usable running implementation, not merely a compiled library | Coordinated TUI build, isolated daemon smoke, reload, and `selfdev status` | Build passed, live workflow passed, and current/shared channels confirmed `a495fb059-dirty-40e6123ec268`. |
| Accurate documentation and reported measurements | Transport-doc source cross-check; all 20 raw values matched against this report; scoped `git diff --check` | Passed. Single-sample caveat retained, experiment limits explicit, links resolve, patch hygiene clean. |
| OAuth live scope is not overstated | Actual daemon attempt with `openai`, then successful `openai-api` attempt | OAuth preparation unavailable with configured credentials. API-key route passed. No live OAuth success claimed and no authentication modified. |
| Steering/multiplexing are not implied by this implementation | Scope and transport-doc review | Explicitly deferred. Neither is counted as implemented or validated. |

Source-check labels refer to assertions run against the production files after
the runtime tests. S1 checks timeout/TTL constants and timer wiring, S2 opt-out
parsing, S3 transport/model exclusions, S4 existing-socket precedence, S5 fork
slot construction, S6 reset clearing, S7/S8 diagnostic outputs, S9 idle ordering,
S10 canary snapshot behavior, S11 send-guard lifetime, and S12 both turn-path
call orders. All 13 assertions passed, counting the two S12 paths separately.

## Whole-result rerun after ledger completion

The mapped checks were rerun on 2026-09-06 at 04:01–04:03 UTC, after the ledger
was committed. No feature code changed during this pass. The checks produced
these outcomes, rather than relying on the earlier completion assessment:

| Mapped checks rerun | Observed outcome |
| --- | --- |
| Full OpenAI runtime suite, including handshake, payload, compatibility, cancellation, expiry, identity and recovery checks | 126 passed, 0 failed, 3 opt-in tests ignored. |
| Both idle-session tests | 2 passed. Busy sessions skipped and pending preparation released the agent lock. |
| Active-provider delegation | 1 passed. No completion invoked during preparation. |
| Opt-in live warmup and continuation | Passed separately. Warmup ready in 550 ms; generated replies correct; prepared socket and subsequent response chain reused. |
| All mapped source assertions and documentation checks | 21 assertions passed, including the original 13 plus default no-op, failure isolation, malformed-event rejection, diagnostic-field review, scope, ledger shape, links and raw-value checks. The scanner initially stopped at a test-only helper; its boundary was corrected and affected assertions rerun successfully. |
| Protocol and all four lifecycle diagnostic outputs | Log re-scan found all five outputs. Ready/hit/miss/unavailable counts were 32/22/2/3, and protocol diagnostics appeared 112 times. These are cumulative log observations, not per-benchmark counts. |
| Full TUI compilation | Passed again in 17.11 seconds. |
| Running shared-server activation | Confirmed the feature-bearing `a495fb059-dirty-40e6123ec268` daemon remained active. The additional build did not need another feature reload. |
| Actual daemon subscription through first generated reply | Passed again: `OK`, `websocket/persistent-reuse`, observed prewarm hit. |
| Live OAuth limitation | Attempted again and remained unavailable: no prepared socket within eight seconds. API-key workflow passed; credentials were not changed to force this check through. |
| Enabled-versus-disabled application experiment | Another 10 alternating pairs completed. All 20 responses correct; warm first-request reuse 10/10, cold reuse 0/10; warm latency lower in 8/10 pairs. |

The repeated experiment's median first-text latency was **1,323.76 ms disabled
versus 1,167.99 ms enabled**, a **155.77 ms (11.8%) reduction**. This independently
repeated the intended benefit: lower foreground latency when preparation overlaps
user think time. Two warmed trials were slower, confirming that a hit is not a
per-request speedup guarantee. The first experiment remains recorded above, not
replaced by a selected subset. Across both runs, all 40 replies were correct and
18 of 20 pairs were faster with prewarming. Host compilation was concurrent with
part of this second run, so these remain local workload observations.

| Follow-up pair | Disabled (ms) | Enabled (ms) |
| --- | ---: | ---: |
| 1 | 1260.24 | 1155.53 |
| 2 | 1229.01 | 1692.87 |
| 3 | 3771.96 | 896.33 |
| 4 | 1345.48 | 937.58 |
| 5 | 1138.15 | 1142.67 |
| 6 | 1302.03 | 934.20 |
| 7 | 1347.84 | 1180.45 |
| 8 | 1535.42 | 1439.54 |
| 9 | 2321.96 | 1188.66 |
| 10 | 1268.40 | 1257.73 |

This pass closes the observed behavior-and-benefit loop for the implemented
API-key workflow. Source-only and blocked checks retain the evidence types and
limitations in the ledger; they are not relabeled as live runtime successes.
