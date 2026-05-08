# Harness Events

`harness-events` is the structured execution-event foundation for the jcode-harness cockpit. The first slice is intentionally local and in-process: it defines a typed event schema and a broadcast EventBus that future NDJSON, replay, SSE, broker, gRPC, and WebSocket adapters can consume.

For consumer-facing examples, CI recipes, transport selection guidance, troubleshooting, and schema stability policy, see [`HARNESS_EVENTS_PROTOCOL.md`](HARNESS_EVENTS_PROTOCOL.md).

## Current scope

Implemented in `src/harness_events.rs`:

- `HARNESS_EVENT_SCHEMA_VERSION = 1`.
- `HarnessEventLevel` with `trace`, `debug`, `info`, `warn`, and `error`.
- `HarnessEventKind` with run, skill, memory, tool, file, test, gate, approval, and control-command events.
- `HarnessEventPayloadClass` with `safe_metadata`, `sensitive_metadata`, `secret`, `user_content`, and `artifact_reference`.
- `HarnessEvent` as the serialized object.
- `HarnessEventDraft` for producer-side construction.
- `HarnessEventBus` for in-process pub/sub fan-out.
- Default payload redaction before events leave the bus.
- Local NDJSON writer/append helpers for already-redacted `HarnessEvent` objects.
- `HarnessControlCommand` plus `harness_control_command_event_draft` for auditable dashboard/WebSocket control commands.
- `HarnessEventSink` / `HarnessEventSource` traits plus NDJSON implementations for future broker adapters.
- Deterministic broker route mapping for NATS subjects, Redis stream keys, and durable consumer names without storing broker IDs in the event schema.

Consumers should subscribe to the in-process bus first, then attach sinks such as NDJSON, SSE, or future WebSocket/broker adapters.

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

Durable NDJSON logs now have explicit local retention controls. `jcode events prune` is safe by default: it reports the logs that would be removed and only deletes files when `--apply` is present.

Examples:

```bash
jcode events prune --keep-logs 50
jcode events prune --keep-logs 50 --json
jcode events prune --keep-logs 50 --max-total-bytes 104857600 --apply
```

Retention policy semantics:

- logs are ordered newest-first by filesystem modification time;
- `--keep-logs N` keeps at most the newest `N` local `.ndjson` logs;
- `--max-total-bytes B` keeps newest logs until adding the next log would exceed `B` bytes;
- `--apply` is required for deletion, otherwise the command is a dry-run report.

Runtime producers also expose conservative sampling knobs for high-volume streams:

```bash
JCODE_HARNESS_EVENTS_MIN_LEVEL=warn jcode run --ndjson "..."
JCODE_HARNESS_EVENTS_TOOL_SAMPLE_EVERY=10 jcode run --ndjson "..."
```

Sampling policy semantics:

- `JCODE_HARNESS_EVENTS_MIN_LEVEL` accepts `trace`, `debug`, `info`, `warn`, or `error` and drops events below that severity before writing the durable log.
- `JCODE_HARNESS_EVENTS_TOOL_SAMPLE_EVERY=N` keeps the first tool event of each sampled kind and then one of every `N` info-level tool events. `1` or an unset value records all tool events.
- `warn` and `error` events are always retained after the minimum-level check so failures are not hidden by tool-event sampling.

Producer guidance until high-volume runtime coverage is broader:

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

The default log directory is under `JCODE_RUNTIME_DIR` when set, otherwise the platform runtime directory, in a `harness-events/` subdirectory. Runtime producer coverage is currently incremental: `jcode run --ndjson` writes typed run/tool summary events and exposes `harness_run_id` plus `harness_event_log` in its `start`, `done`, and `error` records.

## Pluggable sinks, sources, and broker routes

The first #22 slice keeps local-first builds dependency-free while defining the extension seam for NATS JetStream, Redis Streams, or other durable adapters.

Core traits:

- `HarnessEventSink::publish_event(&HarnessEvent)` returns a `HarnessEventSinkAck` with sink name, durability, event id, run id, and optional message id.
- `HarnessEventSource::read_events_after(run_id, last_event_id)` returns replayable events with `Last-Event-ID` semantics.
- `HarnessEventNdjsonSink` and `HarnessEventNdjsonSource` implement those traits over the existing local NDJSON logs.

Broker routing stays outside the event envelope. `harness_event_broker_route(&event)` derives deterministic adapter metadata:

```text
nats_subject:     jcode.harness_events.v1.run.<hex_run_id>[.session.<hex_session_id>][.task.<hex_task_id>]
redis_stream_key: jcode:harness-events:v1:run:<hex_run_id>:events[:session:<hex_session_id>][:task:<hex_task_id>]
durable_consumer: jcode-harness-<hex_run_id>
```

The route encodes run/session/task components as ASCII-safe hex tokens so broker separators and wildcards cannot collide with user-provided ids. Future broker adapters should use this mapping for publish/consume configuration, while still writing local NDJSON evidence first so broker outages do not erase audit trails.

Broker messages use a versioned envelope from `serialize_harness_event_broker_payload`. The current delivery semantics are explicitly `at_least_once`: consumers must deduplicate by the envelope `dedupe_key` (`schema_version` + `event_id`). `HarnessEventDelivery` and `HarnessEventDeliveryAck` carry adapter-neutral message id, redelivery, attempt, and ack outcome metadata so future JetStream and Redis adapters do not need to fork the event schema.

## SSE protocol core

The first SSE slice is transport-neutral framing for future local dashboard endpoints. A `HarnessEvent` maps to a Server-Sent Events message as:

- `Content-Type: text/event-stream`
- `id`: `event_id`, used by browser `Last-Event-ID` resume.
- `event`: the snake_case `HarnessEventKind`, such as `tool_finished`.
- `retry`: optional reconnect delay, default helper value `2000` ms.
- `data`: compact JSON serialization of the already-redacted `HarnessEvent`.

Example frame:

```text
id: hevt_demo
event: tool_finished
retry: 2000
data: {"schema_version":1,"event_id":"hevt_demo","run_id":"run_demo","timestamp":"2026-05-08T04:22:00Z","sequence":3,"level":"info","kind":"tool_finished","payload_class":"safe_metadata","payload":{"tool":"cargo test","status":"ok"}}

```

Browser clients can later consume the local endpoint with standard EventSource APIs:

```js
const events = new EventSource("http://127.0.0.1:PORT/events/runs/run_demo/stream");
events.addEventListener("tool_finished", (event) => {
  const harnessEvent = JSON.parse(event.data);
  console.log(harnessEvent.sequence, harnessEvent.payload.status);
});
events.onerror = () => console.warn("event stream reconnecting");
```

The gateway endpoint is exposed at `GET /events/runs/{urlencoded_run_id}/stream` for paired clients. It reuses `write_harness_event_sse`, `render_harness_event_sse`, and `harness_events_after_last_event_id` so `Last-Event-ID` replays retained local events before subscribing to live fan-out. Use `?replay=only` for scripts/tests that want the retained tail and a closed connection.

## WebSocket control protocol core

The first #24 slice defines the bidirectional command envelope that a WebSocket UI can send without scraping terminal text. Commands are parsed and validated by `HarnessControlCommand`, then converted into redacted/auditable `HarnessEventDraft` values with `harness_control_command_event_draft`.

The gateway bridge now intercepts supported control commands on `/ws` before forwarding normal newline-delimited protocol messages to the Unix-socket bridge. Non-control messages continue through the existing bridge unchanged. Handled control commands receive a JSON acknowledgement frame containing the emitted audit event.

Example approval resolution command:

```json
{
  "command": "resolve_human_approval",
  "run_id": "run_demo",
  "approval_id": "approval_deploy",
  "decision": "approved",
  "actor": "dashboard",
  "reason": "user clicked approve",
  "client_command_id": "cmd_1"
}
```

Auditing behavior:

- `subscribe_events` is read-only and does not require write authorization.
- `resolve_human_approval`, `pause_run`, `resume_run`, `cancel_run`, and `ui_command` require write authorization from the gateway/client layer.
- authorized approval resolutions emit `human_approval_resolved` with `approval_id`, `decision`, `actor`, `reason_present`, and `client_command_id` metadata;
- unauthorized write commands emit `control_command_rejected` at `warn` level;
- free-form reasons are not copied into the audit payload, only `reason_present`, to avoid leaking user text;
- command `args` still pass through the normal redaction rules before publication.
- unsupported command names and ordinary protocol messages are not intercepted and continue through the existing bridge unchanged.

### CLI helpers

`jcode events` exposes the local sink without mixing human text into NDJSON streams:

```bash
jcode events path --run run_123
jcode events path --run run_123 --json
jcode events list --json
jcode events show --run run_123 --json
jcode events tail --run run_123 --lines 50
jcode events tail --run run_123 --lines 50 --ndjson
jcode events export --run run_123 --output run.ndjson --json
jcode events export --run run_123 > run.ndjson
jcode events sse --run run_123 --last-event-id hevt_seen > run.sse
jcode events prune --keep-logs 50 --json
jcode events replay --run run_123 > replay.md
jcode events replay --run run_123 --json > replay.json
jcode events bench --events 10000 --json > harness-events-bench.json
```

- `path` prints the sanitized default log path for a run id.
- `list` indexes local `.ndjson` logs and marks corrupt files instead of hiding them.
- `show` prints summary metadata for one run.
- `tail --ndjson` writes only raw event NDJSON to stdout.
- `export` validates each source line as `HarnessEvent` before rewriting normalized NDJSON.
- `export --json` requires `--output` so stdout remains machine-safe.
- `sse` validates the local log and writes EventSource-compatible SSE frames to stdout or `--output`; `--last-event-id` emits only events after a retained event id.
- `prune` reports retention candidates by newest-first log order; pass `--apply` to actually delete local event logs.
- `replay` reconstructs a local audit timeline as Markdown by default, or JSON with `--json`. Replay output includes phase grouping, elapsed milliseconds, parent event references, child counts, duration hints, and explicit failure points.

Replay and indexing use a tolerant read report for auditability: valid event lines are retained, invalid or truncated lines are surfaced as line-numbered diagnostics, and JSON replay includes a `diagnostics` array alongside `summary`, `timeline`, and `events`. Strict NDJSON consumers such as `tail --ndjson` and `export` still fail on malformed input so automation does not silently consume damaged streams.

### Performance baseline

`jcode events bench` runs a dependency-free synthetic baseline for #25. It measures event publish with no subscribers, NDJSON serialization to memory, NDJSON write/read through a temporary file, and replay timeline construction. The report is intentionally a baseline, not a hard CI threshold yet, because numbers depend on machine, build profile, filesystem, and event payload size.

Recommended use while tuning defaults:

```bash
cargo run -q -p jcode --bin jcode -- events bench --events 10000 --json \
  > harness-events-bench.json
```

Compare reports on the same machine/profile before changing buffer capacities, flush behavior, retention, or sampling. The initial budget guidance is: no-subscriber publish must stay non-blocking, NDJSON writes flush safely without fsync by default, replay parsing must report diagnostics rather than panic, and long-running modes must bound in-memory buffers.

Slow subscribers are intentionally backpressured by Tokio broadcast semantics instead of blocking publishers: each in-process subscriber reads from a bounded ring, lagging receivers observe `Lagged` and may miss old in-memory events. Durable consumers that cannot lose events should attach to the NDJSON sink or a future broker-backed transport rather than relying on the in-memory fan-out ring alone.

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
