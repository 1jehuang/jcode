# Harness Events

`harness-events` is the structured execution-event foundation for the jcode-harness cockpit. The first slice is intentionally local and in-process: it defines a typed event schema and a broadcast EventBus that future NDJSON, replay, SSE, broker, gRPC, and WebSocket adapters can consume.

## Current scope

Implemented in `src/harness_events.rs`:

- `HARNESS_EVENT_SCHEMA_VERSION = 1`.
- `HarnessEventLevel` with `trace`, `debug`, `info`, `warn`, and `error`.
- `HarnessEventKind` with initial run, skill, memory, tool, file, test, gate, and approval events.
- `HarnessEventPayloadClass` with `safe_metadata`, `sensitive_metadata`, `secret`, `user_content`, and `artifact_reference`.
- `HarnessEvent` as the serialized object.
- `HarnessEventDraft` for producer-side construction.
- `HarnessEventBus` for in-process pub/sub fan-out.
- Default payload redaction before events leave the bus.
- Local NDJSON writer/append helpers for already-redacted `HarnessEvent` objects.

External transports are out of scope for this first slice. Consumers should subscribe to the in-process bus first, then later attach sinks such as NDJSON or SSE.

## Event object

Example:

```json
{
  "schema_version": 1,
  "event_id": "hevt_1778209200000_123",
  "run_id": "run_demo",
  "session_id": "session_demo",
  "parent_event_id": "hevt_parent",
  "timestamp": "2026-05-08T03:00:00Z",
  "sequence": 7,
  "level": "info",
  "kind": "tool_finished",
  "payload_class": "safe_metadata",
  "payload": {
    "tool": "cargo test",
    "status": "passed"
  }
}
```

Common fields:

- `schema_version`: event schema version. Increment only for incompatible changes.
- `event_id`: unique event identifier.
- `run_id`: stable run/correlation identifier.
- `session_id`: optional session that owns or observed the event.
- `parent_event_id`: optional parent for nested operations.
- `timestamp`: UTC timestamp.
- `sequence`: monotonic per-`run_id` sequence assigned by `HarnessEventBus`.
- `level`: severity/verbosity level.
- `kind`: typed event kind.
- `payload_class`: producer-declared sensitivity class. `secret` and `user_content` payloads are redacted wholesale by default.
- `payload`: structured metadata. Keep it redacted and reference artifacts instead of embedding large or sensitive content.

## Privacy and redaction

The core bus redacts payloads before publishing. This is intentionally enforced at the producer-to-bus boundary so future NDJSON, replay, SSE, broker, gRPC, or WebSocket sinks receive already-redacted events by default.

Current rules:

- `secret` and `user_content` payload classes are replaced with a small redaction marker.
- Sensitive object keys are redacted recursively, including token/API-key/password/auth/cookie/prompt/input/stdout/stderr/tool-output style names.
- Secret-looking string values such as `Bearer ...`, `ghp_...`, `github_pat_...`, `sk-...`, and PEM private-key material are redacted even when the key is not known.
- Long strings are truncated to a bounded length and suffixed with `...[truncated]`.
- Safe metadata remains available when it does not match a redaction rule.

Examples:

```json
{
  "payload_class": "safe_metadata",
  "payload": {
    "tool": "deploy",
    "api_key": "[redacted]",
    "nested": { "Authorization": "[redacted]", "safe": "metadata" }
  }
}
```

```json
{
  "payload_class": "user_content",
  "payload": {
    "redacted": true,
    "payload_class": "user_content"
  }
}
```

## Retention and sampling status

This first privacy slice is in-memory only. The core `HarnessEventBus` does not write durable event logs, so there is no persistent retention store yet. Its broadcast ring is bounded by capacity and old in-memory events are dropped by Tokio broadcast semantics when receivers lag.

Retention, cleanup, and sampling policies for durable logs should be implemented with #18/#19. Until then:

- do not store raw event payloads outside the bus;
- prefer artifact references over inline content;
- classify prompts, file contents, raw tool stdout/stderr, and secrets as `user_content` or `secret`;
- keep high-volume events at `trace`/`debug` for future sampling knobs.

## NDJSON local sink

The first NDJSON slice is a low-level sink API for redacted `HarnessEvent` objects:

```rust
use jcode::harness_events::{
    HarnessEventBus,
    HarnessEventDraft,
    append_harness_event_ndjson,
    harness_event_log_path,
};

let bus = HarnessEventBus::global();
let event = bus.publish(HarnessEventDraft::run_started("run_123"));
let path = harness_event_log_path("run_123");
append_harness_event_ndjson(&path, &event)?;
```

Sink guarantees:

- one compact JSON object per line;
- every line is independently parseable as `HarnessEvent`;
- parent directories are created on append;
- file names are derived from sanitized `run_id` values;
- payloads have already passed through the core redaction path.

The default log directory is under `JCODE_RUNTIME_DIR` when set, otherwise the platform runtime directory, in a `harness-events/` subdirectory. Full CLI tail/export commands remain follow-up work for #18.

## Minimal producer usage

```rust
use jcode::harness_events::{HarnessEventBus, HarnessEventDraft};

let bus = HarnessEventBus::global();
let started = bus.publish(HarnessEventDraft::run_started("run_123"));
let completed = bus.publish(HarnessEventDraft::run_completed("run_123"));
assert_eq!(started.sequence, 1);
assert_eq!(completed.sequence, 2);
```

## Design constraints

- Local-first: no broker or network service is required.
- Non-blocking fan-out: slow or absent subscribers must not fail event production.
- Versioned schema: later sinks should rely on `schema_version` and typed `kind`.
- Privacy-first payloads: raw prompts, secrets, large file contents, and unredacted tool output should be classified and are redacted by default before publication.

## Follow-up issues

- #18: NDJSON event log and CLI streaming sink.
- #19: replayable event store and audit timeline.
- #20: SSE endpoint for live dashboard progress.
- #21: redaction, retention, sampling, and privacy policy.
- #22: optional NATS JetStream and Redis Streams adapters.
- #23: gRPC distributed agent control plane prototype.
- #24: WebSocket interactive control channel.
- #25: benchmarks, load tests, and event overhead budgets.
- #26: full protocol guide, examples, and CI recipes.
