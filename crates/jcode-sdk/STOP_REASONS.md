# Abnormal turn outcomes

Harness API minor 8 advertises `turn_stop_reasons`. Rust consumers can import
`ApiEvent` and `TurnStopReason` directly from `jcode_sdk`.

```json
{"v":1,"ev":"turn_stopped","session_id":"s1","reason":"interrupted","message":"The turn was interrupted by a cancellation request."}
```

`reason` is `interrupted`, `failure`, `crash`, `provider_guardrail`, or
`limit_reached`. Rust maps future unknown reasons to `TurnStopReason::Unknown`.
Optional `provider_stop_reason` preserves the provider's raw reason, such as
`refusal`. Display `message` as a status notice, not assistant output.

Natural completion emits only the existing `turn_done`. Abnormal completion emits
`turn_stopped` before `turn_done`. Failures and caught panics retain the legacy
`error` between those two events. Text framing events may also occur before the
final `turn_done`. Consumers should avoid duplicating the stop explanation when
also handling the legacy error. Request/control errors are not turn stops.

The Rust and TypeScript `run` convenience methods retain cancellation/guardrail
outcomes in `stop_reason` / `stopReason` and `stop_message` / `stopMessage`.
Failures still return an error, with the structured stop available to `on_event`
/ `onEvent` and event subscribers before that error.

A caught runtime panic is `crash`. Socket loss alone is not proof of a crash:
continue using SDK disconnect diagnostics for transport/process termination.
These events are live and are not persisted for replay after reconnect. Existing
session status/recovery information remains the fallback after reconnect or when
connected to an older runtime. Capability advertisement describes bridge support,
not a guarantee that every older daemon supplies structured runtime outcomes.
