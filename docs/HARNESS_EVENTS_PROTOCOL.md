# Harness Events Protocol Guide

`harness-events` is the local-first execution event protocol for jcode-harness. It gives users, scripts, dashboards, CI jobs, and future swarms one consistent way to inspect what happened during a run without scraping terminal text.

This guide is the consumer-facing companion to [`HARNESS_EVENTS.md`](HARNESS_EVENTS.md), which documents the Rust implementation details.

## Quick start

Run one command and inspect the event stream path:

```bash
jcode events path --run run_demo --json
```

For real headless runs, `jcode run --ndjson` includes `harness_run_id` and `harness_event_log` in its `start`, `done`, and `error` records when typed harness event logging is active:

```bash
jcode run --ndjson "summarize this repo" | tee run.ndjson
```

Then inspect the local event log:

```bash
RUN_ID=$(jq -r 'select(.type == "start") | .harness_run_id' run.ndjson | head -1)
jcode events show --run "$RUN_ID" --json
jcode events replay --run "$RUN_ID" > replay.md
jcode events tail --run "$RUN_ID" --lines 20 --ndjson
jcode events sse --run "$RUN_ID" --last-event-id hevt_seen > replay.sse
```

If you already have a known synthetic or session run id, these commands are safe to run offline:

```bash
jcode events list --json
jcode events replay --run run_demo --json
jcode events bench --events 10000 --json > harness-events-bench.json
```

## Event schema reference

Every event is one JSON object with a stable common envelope:

```json
{
  "schema_version": 1,
  "event_id": "hevt_demo",
  "run_id": "run_demo",
  "session_id": "session_demo",
  "parent_event_id": "hevt_parent",
  "timestamp": "2026-05-08T04:22:00Z",
  "sequence": 3,
  "level": "info",
  "kind": "tool_finished",
  "payload_class": "safe_metadata",
  "payload": {
    "tool": "cargo test",
    "status": "ok",
    "duration_ms": 1200
  }
}
```

Common fields:

| Field | Meaning | Stability |
| --- | --- | --- |
| `schema_version` | Integer schema version. Current value is `1`. | Increment only for incompatible envelope changes. |
| `event_id` | Unique event id. Also used as SSE `id`. | Stable per event. |
| `run_id` | Correlation id for a run/session/log. | Stable for a run. |
| `session_id` | Optional owning/observing session. | Optional. |
| `parent_event_id` | Optional parent operation id. | Optional. |
| `timestamp` | UTC event time. | RFC3339/chrono JSON format. |
| `sequence` | Monotonic sequence per `run_id`. | Assigned by `HarnessEventBus`. |
| `level` | `trace`, `debug`, `info`, `warn`, or `error`. | Additive. |
| `kind` | Typed event kind, such as `run_started` or `tool_finished`. | Additive. |
| `payload_class` | Declared sensitivity class. | Additive. |
| `payload` | Redacted structured metadata. | Kind-specific, additive by key. |

Initial event kinds include run start/completion/failure, skill selection, memory lookup start/finish, tool start/finish, file changed, test start/pass/fail, gate pass/fail, and human approval required.

## Privacy and redaction

The event bus redacts before events reach NDJSON, replay, SSE, or future transports. Consumers should assume payloads are safe for local audit, but producers should still avoid embedding raw prompts, file contents, secrets, or full tool output.

Current policy:

- `secret` and `user_content` payload classes are replaced wholesale with a redaction marker.
- Sensitive keys such as API key, authorization, token, password, cookie, prompt, input, stdout, stderr, and tool output are recursively redacted.
- Secret-looking values such as `Bearer ...`, `ghp_...`, `github_pat_...`, `sk-...`, and PEM private key material are redacted even under unknown keys.
- Long strings are truncated and suffixed with `...[truncated]`.
- Safe token metrics such as `input_tokens` and `output_tokens` remain visible.

Example redacted payload:

```json
{
  "payload_class": "safe_metadata",
  "payload": {
    "tool": "deploy",
    "api_key": "[redacted]",
    "input_tokens": 1234
  }
}
```

## NDJSON local logs

NDJSON is the best default for CLI tools, CI jobs, dashboards that tail local files, and external automations.

Properties:

- one compact JSON event per line;
- stdout-safe for `tail --ndjson` and `export`;
- strict export/tail fail on malformed source logs;
- replay/list/show use tolerant read reports so valid prefixes remain inspectable;
- default directory is `$JCODE_RUNTIME_DIR/harness-events/` when `JCODE_RUNTIME_DIR` is set, otherwise the platform runtime directory.

Useful commands:

```bash
jcode events path --run "$RUN_ID"
jcode events list --json
jcode events show --run "$RUN_ID" --json
jcode events tail --run "$RUN_ID" --lines 50 --ndjson
jcode events export --run "$RUN_ID" --output harness-events.ndjson --json
```

A minimal NDJSON stream looks like:

```jsonl
{"schema_version":1,"event_id":"hevt_start","run_id":"run_demo","timestamp":"2026-05-08T04:22:00Z","sequence":1,"level":"info","kind":"run_started","payload_class":"safe_metadata","payload":{"status":"started"}}
{"schema_version":1,"event_id":"hevt_tool","run_id":"run_demo","parent_event_id":"hevt_start","timestamp":"2026-05-08T04:22:01Z","sequence":2,"level":"info","kind":"tool_finished","payload_class":"safe_metadata","payload":{"tool":"cargo test","status":"ok","duration_ms":1000}}
{"schema_version":1,"event_id":"hevt_done","run_id":"run_demo","timestamp":"2026-05-08T04:22:02Z","sequence":3,"level":"info","kind":"run_completed","payload_class":"safe_metadata","payload":{"status":"ok","duration_ms":2000}}
```

## Replay and audit

Replay reconstructs a local timeline without contacting providers or external services:

```bash
jcode events replay --run "$RUN_ID" > replay.md
jcode events replay --run "$RUN_ID" --json > replay.json
```

Markdown replay contains:

- summary status and duration;
- diagnostics for corrupt/truncated lines;
- failure points;
- timeline grouped by run, planning, memory, tool execution, files, tests, gates, approval, and completion.

JSON replay contains:

- `summary`: status, event count, first/last timestamp, duration, path, diagnostics summary;
- `timeline`: derived phase, elapsed milliseconds, parent id, child count, status, failure flag, details;
- `diagnostics`: line-numbered parse/read diagnostics;
- `events`: original redacted events.

Corrupt or partial logs are not hidden. For example, a truncated line is reported with its line number while valid prefix events are still replayed.

## SSE dashboard stream

SSE is the planned one-way live dashboard transport. It is simpler than WebSocket for server-to-client progress because browsers support automatic reconnect through `EventSource` and `Last-Event-ID`.

The protocol core is implemented and endpoint wiring is tracked by #20. A `HarnessEvent` maps to:

```text
id: hevt_tool
event: tool_finished
retry: 2000
data: {"schema_version":1,"event_id":"hevt_tool","run_id":"run_demo","timestamp":"2026-05-08T04:22:01Z","sequence":2,"level":"info","kind":"tool_finished","payload_class":"safe_metadata","payload":{"tool":"cargo test","status":"ok"}}

```

Browser example for the future local endpoint:

```js
const source = new EventSource("http://127.0.0.1:PORT/events/runs/run_demo/stream");
source.addEventListener("tool_finished", (event) => {
  const harnessEvent = JSON.parse(event.data);
  console.log(harnessEvent.sequence, harnessEvent.payload.tool);
});
source.onerror = () => console.warn("stream reconnecting");
```

Reconnect behavior should use `Last-Event-ID` to replay retained local events after the last observed `event_id`, then subscribe to live fan-out. Slow dashboard clients must not block agent execution; lagging in-memory subscribers may miss old events and should use NDJSON replay for durable audit.

Until the authenticated local endpoint lands, scripts and dashboard prototypes can validate the exact wire format from local logs:

```bash
jcode events sse --run "$RUN_ID" --retry-ms 2000 > replay.sse
jcode events sse --run "$RUN_ID" --last-event-id hevt_tool > replay-tail.sse
```

`events sse` is strict like `events export`: malformed NDJSON fails the command so prototypes do not silently consume a damaged stream. Use `events replay --json` when you need tolerant diagnostics for corrupt or partial logs.

The gateway SSE endpoint is now available for paired dashboard clients:

```bash
curl -N \
  -H "Authorization: Bearer $JCODE_GATEWAY_TOKEN" \
  -H "Last-Event-ID: hevt_seen" \
  "http://127.0.0.1:7643/events/runs/$RUN_ID/stream"
```

Endpoint details:

- path: `GET /events/runs/{urlencoded_run_id}/stream`
- auth: same paired-device gateway token as `/ws`, via `Authorization: Bearer <token>` or `?token=<token>` for browser prototypes;
- replay: reads the retained local NDJSON log, applies `Last-Event-ID`, and sends matching SSE frames before live fan-out;
- live mode: subscribes to the in-process `HarnessEventBus` and streams matching `run_id` events;
- test/export mode: `?replay=only` or `?once=1` sends only the retained replay tail and closes;
- retry tuning: `?retry_ms=2000` overrides the SSE `retry:` field when greater than zero.

## Choosing a transport

| Use case | Recommended transport | Why |
| --- | --- | --- |
| CLI scripts and CI artifacts | NDJSON | Stable, grep/jq-friendly, no server required. |
| Local audit and evidence review | Replay Markdown/JSON | Summarizes phases, failures, durations, and diagnostics. |
| Browser dashboard live progress | SSE | One-way server-to-client, EventSource reconnect, normal HTTP. |
| Interactive dashboard control | WebSocket | Needed for bidirectional commands and approvals. |
| Multi-agent queues and replayable fan-out | NATS/Redis Streams | Durable brokered delivery and work queues. |
| Distributed typed agent RPC | gRPC | Strong contracts for remote workers/control plane. |

Start with NDJSON/replay. Add SSE when you need live visual progress. Use WebSocket only when the UI must send real-time control messages.

## Broker adapter seam

NATS JetStream and Redis Streams remain optional future adapters. The core build has no broker dependency; it exposes a small sink/source seam over `HarnessEvent` so adapters can be feature-gated later.

Current local implementations:

- `HarnessEventSink` publishes a redacted event and returns `HarnessEventSinkAck`.
- `HarnessEventSource` reads replayable events after an optional last event id.
- `HarnessEventNdjsonSink` and `HarnessEventNdjsonSource` provide the local fallback every broker-backed path should preserve.
- `HarnessEventMemoryBroker` is the dependency-free adapter harness for tests and future adapter development: it serializes through the broker envelope, exposes delivery metadata, tracks ack outcomes, and can redeliver unacked messages with incremented attempts.
- `HarnessEventFanoutSink` enforces the outage policy for future adapters: write local audit evidence first, publish to the broker second, and record broker failure as non-fatal evidence unless strict mode is explicitly enabled.
- `HarnessEventRedisStreamSink` is available behind `--features harness-events-redis`; it publishes the broker envelope to Redis Streams with `XADD`, using the derived stream key and storing searchable fields such as `event_id`, `run_id`, `dedupe_key`, `schema_version`, `kind`, and `level` beside the JSON payload.
- `HarnessEventRedisStreamSource` is also feature-gated. It polls configured Redis stream keys with bounded `XRANGE`, parses entries back into `HarnessEventDelivery`, validates searchable metadata against the envelope payload, and exposes `XACK` behavior through `harness_event_redis_stream_ack_command` / `ack_delivery`.

Route mapping for future adapters:

| Adapter | Mapping |
| --- | --- |
| NATS subject | `jcode.harness_events.v1.run.<hex_run_id>[.session.<hex_session_id>][.task.<hex_task_id>]` |
| Redis stream key | `jcode:harness-events:v1:run:<hex_run_id>:events[:session:<hex_session_id>][:task:<hex_task_id>]` |
| Durable consumer | `jcode-harness-<hex_run_id>` |

Route metadata is derived with `harness_event_broker_route(&event)` and must not be written back into the event schema. Run, session, and task ids are hex-encoded before entering subjects/keys so broker separators and wildcards cannot collide with user-provided ids. Broker outage policy should be: write local NDJSON first, attempt broker publish second, surface broker failures as warnings/evidence instead of deleting or rewriting local proof.

Broker payload and ack contract:

- `serialize_harness_event_broker_payload` wraps a redacted `HarnessEvent` in a versioned envelope.
- `delivery_semantics` is currently `at_least_once`, never exactly-once.
- consumers must deduplicate by `dedupe_key`, which is derived from `schema_version` and `event_id`.
- `HarnessEventDelivery` carries route, message id, redelivery flag, and attempt count for adapter-specific consumers.
- `HarnessEventDeliveryAck` records `pending`, `acked`, `nacked`, `redelivered`, or `dropped` outcomes without promising that NATS and Redis have identical ack semantics.
- JetStream publish ack should mean broker persistence; consumer ack should mean processing completed.
- Redis `XADD` id should map to `message_id`; `XACK` only applies when consumer groups are used. The helper policy is: `acked` and `dropped` outcomes call `XACK`, while `nacked` remains pending for retry or redelivery.
- The memory broker is not durable and should not be used as audit evidence; use it to verify adapter-independent publish/consume/ack behavior before wiring NATS or Redis.
- Broker fanout must never delete or rewrite local NDJSON evidence. In normal mode broker publish errors are captured in `HarnessEventFanoutReport.broker_error`; strict mode may return an error after the local write has already succeeded.
- Redis support remains opt-in. The default build has no Redis dependency. The live Redis round-trip test is gated on both the cargo feature and `JCODE_HARNESS_EVENTS_REDIS_URL`; when the env var is unset, the test logs a skip message and exits successfully.

Redis operational policy for this MVP:

- Backpressure is bounded at the source boundary with `DEFAULT_HARNESS_EVENT_REDIS_STREAM_READ_COUNT` and per-call `XRANGE ... COUNT`; worker loops should page rather than read unbounded streams.
- Reconnect is caller-managed: a Redis command error is surfaced as evidence/error, and the caller should rebuild the sink/source from the configured URL on the next retry. Local NDJSON fanout remains the source of audit truth during broker outages.
- Consumer-group processing can use the same delivery parser and ack command policy when the future `XREADGROUP` worker loop lands.

## WebSocket control command core

The #24 control-channel foundation is transport-neutral: a WebSocket client sends a JSON command, the gateway validates read/write authorization, and the core converts the command into a redacted audit event. This keeps dashboard control auditable even before a full browser UI exists.

On `/ws`, the gateway now intercepts supported command names before forwarding ordinary newline-delimited protocol messages to the existing Unix-socket bridge. A handled command returns a JSON response frame of type `harness_control_ack`, `harness_control_rejected`, or `harness_control_error` with the emitted audit event when available.

Command envelope examples:

```json
{"command":"subscribe_events","run_id":"run_demo","last_event_id":"hevt_seen"}
```

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

Supported MVP commands:

| Command | Authorization | Audit event |
| --- | --- | --- |
| `subscribe_events` | read-only | `control_command_received` |
| `resolve_human_approval` | write | `human_approval_resolved` when authorized |
| `pause_run` | write | `control_command_received` |
| `resume_run` | write | `control_command_received` |
| `cancel_run` | write | `control_command_received` |
| `ui_command` | write | `control_command_received` |

Unauthorized write commands must be rejected by the gateway and audited as `control_command_rejected` at `warn` level. Approval reasons are intentionally summarized as `reason_present` instead of copied verbatim, and all command metadata still passes through the normal payload redaction path.

## CI recipe

A GitHub Actions-style workflow can collect event evidence as artifacts:

```yaml
name: harness-events-smoke
on: [pull_request]
jobs:
  smoke:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo build -p jcode --bin jcode
      - name: Run event benchmark baseline
        run: |
          ./target/debug/jcode events bench --events 1000 --json \
            > harness-events-bench.json
      - name: Validate harness event tests
        run: cargo test -p jcode harness_events --lib
      - uses: actions/upload-artifact@v4
        with:
          name: harness-events-evidence
          path: |
            harness-events-bench.json
```

For provider-backed runs, also capture the `jcode run --ndjson` output and export the referenced `harness_event_log` path as an artifact.

## Troubleshooting

| Symptom | What to check | Fix |
| --- | --- | --- |
| `No harness event logs found.` | `JCODE_RUNTIME_DIR` and run id. | Use `jcode events path --run <id>` and verify the file exists. |
| `status: corrupt` in `events list` | The first readable event could not be parsed. | Inspect the `error` field, then keep the raw file as evidence. |
| `status: partial` | Some valid events were read, but at least one line was invalid/truncated. | Use `events replay --json` and inspect `diagnostics`. |
| `tail --ndjson` or `export` fails | Strict machine path detected malformed NDJSON. | Use `replay` for tolerant audit, fix or quarantine the damaged log before automation consumes it. |
| Sensitive value appears risky | Producer likely used too broad a safe metadata key. | Classify as `secret` or `user_content`, or add a redaction key rule. |
| Live dashboard missed events | In-memory subscriber lagged. | Resume from NDJSON/replay with `Last-Event-ID` or future broker transport. |

## Stability policy

- `schema_version = 1` is the current event envelope version.
- Additive enum variants and payload keys are allowed without changing `schema_version`.
- Consumers must ignore unknown payload keys and unknown future event kinds where possible.
- Incompatible envelope changes require incrementing `schema_version` and documenting migration.
- Payload values are redacted before transport, so sinks must not attempt to recover raw secrets.
- `event_id`, `run_id`, `timestamp`, `sequence`, `level`, `kind`, `payload_class`, and `payload` are the minimum fields consumers should expect for schema version 1.

## Roadmap links

- Epic: #16
- NDJSON sink and CLI: #18
- Replay/audit: #19
- SSE endpoint: #20
- Privacy/redaction: #21
- Brokers: #22
- gRPC: #23
- WebSocket control: #24
- Benchmarks: #25
- Protocol docs: #26
