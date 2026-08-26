// Tests for the telemetry worker's dual-write + D1 self-defense behavior.
// Run with: node --test test/
//
// The worker module is plain ESM with injected bindings (env.DB, env.FIREHOSE),
// so it can be exercised without wrangler by passing mocks.
import test from "node:test";
import assert from "node:assert/strict";

import worker from "../src/worker.js";

const EVENT_URL = "https://telemetry.example/v1/event";
const HEALTH_URL = "https://telemetry.example/v1/health";
const TRANSCRIPT_URL = "https://telemetry.example/v1/transcript";

function makeBody(overrides = {}) {
  return {
    id: "11111111-2222-3333-4444-555555555555",
    event: "onboarding_step",
    version: "0.0.0-test",
    os: "linux",
    arch: "x86_64",
    step: "auth_failed",
    auth_provider: "testprov",
    auth_method: "oauth",
    auth_failure_reason: "callback_timeout",
    ...overrides,
  };
}

function makeDiscoveryBody(overrides = {}) {
  return makeBody({
    event: "discovery",
    event_id: "discovery-event-1",
    session_id: "session-1",
    request_id: "11111111-2222-4333-8444-555555555555",
    phase: "browse",
    category: "payments",
    selected_tool: null,
    outcome: "success",
    failure_reason: null,
    http_status: 200,
    latency_ms: 125,
    response_bytes: 2048,
    result_count: 3,
    query_present: true,
    reason_present: true,
    custom_endpoint: false,
    benchmark_run: true,
    ...overrides,
  });
}

function makeTodoSessionBody(overrides = {}) {
  const correlationId = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
  return makeBody({
    id: correlationId,
    event: "todo_session",
    event_id: "todo-session-event-1",
    correlation_id: correlationId,
    session_end_reason: "normal_exit",
    todos_created: 4,
    todos_completed: 3,
    todos_abandoned: 1,
    todo_updates: 6,
    groups_completed: 2,
    groups_total: 3,
    max_todo_list_size: 4,
    confidence_min: 70,
    confidence_mean: 82.5,
    confidence_count: 4,
    completion_confidence_min: 96,
    completion_confidence_mean: 98,
    completion_confidence_count: 3,
    understands_user_intent_min: 95,
    understands_user_intent_mean: 95,
    understands_user_intent_count: 1,
    closed_feedback_loop_min: 85,
    closed_feedback_loop_mean: 92.5,
    closed_feedback_loop_count: 2,
    end_to_end_ownership_min: 96,
    end_to_end_ownership_mean: 98,
    end_to_end_ownership_count: 2,
    ...overrides,
  });
}

function postRequest(body, url = EVENT_URL) {
  return new Request(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

function makeTranscriptBody(overrides = {}) {
  return {
    id: "11111111-2222-4333-8444-555555555555",
    event: "transcript",
    upload_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    consent_version: 1,
    schema_version: 6,
    version: "0.0.0-test",
    os: "linux",
    arch: "x86_64",
    provider: "test-provider",
    model: "test-model",
    end_reason: "normal_exit",
    message_count: 1,
    messages: [{ role: "user", content: [{ type: "text", text: "private prompt" }] }],
    ...overrides,
  };
}

function makeR2() {
  const puts = [];
  const deletes = [];
  return {
    puts,
    deletes,
    async put(key, value, options) { puts.push({ key, value, options }); },
    async delete(key) { deletes.push(key); },
  };
}

// Minimal D1 mock. `plan` lets tests fail specific statements or set the
// reported database size.
function makeDb(plan = {}) {
  const executed = [];
  const sizeAfter = plan.sizeAfter ?? 1000;
  return {
    executed,
    prepare(sql) {
      return {
        bind(...values) {
          return {
            async run() {
              executed.push({ sql, values });
              if (plan.failInserts && /^INSERT/i.test(sql.trim())) {
                throw new Error(plan.failureMessage || "generic transient error");
              }
              return { meta: { changes: 1, size_after: sizeAfter } };
            },
            async all() {
              executed.push({ sql, values });
              return { results: [] };
            },
          };
        },
        async run() {
          executed.push({ sql, values: [] });
          return { meta: { changes: 0, size_after: sizeAfter } };
        },
        async all() {
          executed.push({ sql, values: [] });
          // PRAGMA table_info: report every column the worker may reference.
          if (/table_info\(web_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "path", "referrer", "visitor_id", "utm_source",
                "utm_medium", "utm_campaign", "cta", "metric_name",
                "metric_value", "rating", "error_kind", "pageview_id",
                "conversion_id", "placement", "install_method",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info\(install_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "conversion_id", "stage", "outcome", "source",
                "placement", "install_method", "failure_stage",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info\(discovery_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "request_id", "phase", "category", "selected_tool",
                "outcome", "failure_reason", "http_status", "latency_ms",
                "response_bytes", "result_count", "query_present",
                "reason_present", "custom_endpoint", "benchmark_run",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info\(todo_session_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "correlation_id", "session_end_reason",
                "todos_created", "todos_completed", "todos_abandoned", "todo_updates",
                "groups_completed", "groups_total", "max_todo_list_size",
                "confidence_min", "confidence_mean", "confidence_count",
                "completion_confidence_min", "completion_confidence_mean",
                "completion_confidence_count", "understands_user_intent_min",
                "understands_user_intent_mean", "understands_user_intent_count",
                "closed_feedback_loop_min", "closed_feedback_loop_mean",
                "closed_feedback_loop_count", "end_to_end_ownership_min",
                "end_to_end_ownership_mean", "end_to_end_ownership_count",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info\(session_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "max_concurrent_sessions", "multi_sessioned",
                "tool_cat_read_search", "tool_cat_write", "tool_cat_other",
                "tool_cat_todo", "feature_todo_used",
                "todo_gate_ownership_count", "todo_gate_hill_count",
                "todo_gate_completion_count", "todo_gate_spike_count",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info\(turn_details\)/.test(sql)) {
            return {
              results: [
                "event_id", "turn_index", "turn_success",
                "tool_cat_read_search", "tool_cat_write", "tool_cat_other",
                "tool_cat_todo", "feature_todo_used",
                "todo_gate_ownership_count", "todo_gate_hill_count",
                "todo_gate_completion_count", "todo_gate_spike_count",
              ].map((name) => ({ name })),
            };
          }
          if (/table_info/.test(sql)) {
            return {
              results: [
                "telemetry_id", "event", "version", "os", "arch", "step",
                "auth_provider", "auth_method", "auth_failure_reason",
                "milestone_elapsed_ms", "event_id", "session_id",
                "schema_version", "build_channel", "is_git_checkout", "is_ci",
                "ran_from_cargo", "account_id", "tier", "model_start",
              ].map((name) => ({ name })),
            };
          }
          return { results: [] };
        },
      };
    },
  };
}

test("consented transcript is stored in private R2 with D1 metadata", async () => {
  const db = makeDb();
  const r2 = makeR2();
  const response = await worker.fetch(
    postRequest(makeTranscriptBody(), TRANSCRIPT_URL),
    { DB: db, TRANSCRIPTS: r2 },
    {},
  );

  assert.equal(response.status, 200);
  assert.equal(r2.puts.length, 1);
  assert.match(r2.puts[0].key, /^transcripts\/\d{4}-\d{2}\/aaaaaaaa-/);
  assert.match(r2.puts[0].value, /private prompt/);
  assert.equal(r2.puts[0].options.customMetadata.consent_version, "1");
  assert.ok(db.executed.some(({ sql }) => /INSERT INTO transcript_uploads/.test(sql)));
});

test("transcript storage redacts credentials but preserves ordinary code", async () => {
  const r2 = makeR2();
  const secret = "sk-ant-oat01-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
  const bearer = "Bearer abcdefghijklmnopqrstuvwxyz0123456789";
  const code = "fn add(a: i32, b: i32) -> i32 { a + b }";
  const body = makeTranscriptBody({
    messages: [{
      role: "user",
      content: [{
        type: "tool_use",
        input: {
          source: code,
          api_key: secret,
          command: `curl -H 'Authorization: ${bearer}'\n${code}`,
        },
      }],
    }],
  });

  const response = await worker.fetch(
    postRequest(body, TRANSCRIPT_URL),
    { DB: makeDb(), TRANSCRIPTS: r2 },
    {},
  );
  assert.equal(response.status, 200);
  const stored = r2.puts[0].value;
  assert.ok(!stored.includes(secret));
  assert.ok(!stored.includes("abcdefghijklmnopqrstuvwxyz0123456789"));
  assert.match(stored, /\[REDACTED_SECRET\]/);
  assert.match(stored, /fn add\(a: i32, b: i32\)/);
});

test("transcript endpoint rejects missing explicit consent version", async () => {
  const response = await worker.fetch(
    postRequest(makeTranscriptBody({ consent_version: 0 }), TRANSCRIPT_URL),
    { DB: makeDb(), TRANSCRIPTS: makeR2() },
    {},
  );
  assert.equal(response.status, 400);
  assert.match(await response.text(), /Unsupported consent version/);
});

test("transcript endpoint fails closed when private storage is unavailable", async () => {
  const response = await worker.fetch(
    postRequest(makeTranscriptBody(), TRANSCRIPT_URL),
    { DB: makeDb() },
    {},
  );
  assert.equal(response.status, 503);
});

test("transcript endpoint rejects declared oversized payload before parsing", async () => {
  const request = postRequest(makeTranscriptBody(), TRANSCRIPT_URL);
  request.headers.set("content-length", String(9 * 1024 * 1024));
  const response = await worker.fetch(request, { DB: makeDb(), TRANSCRIPTS: makeR2() }, {});
  assert.equal(response.status, 413);
});

function makeFirehose() {
  const points = [];
  return {
    points,
    writeDataPoint(point) {
      points.push(point);
    },
  };
}

function makeCtx() {
  const waited = [];
  return {
    waited,
    waitUntil(promise) {
      waited.push(promise);
    },
  };
}

// Position of `column` in an `INSERT ... (col1, col2, ...) VALUES` statement,
// matching the bound values array. Returns -1 when the column is absent.
function columnIndex(sql, column) {
  const match = sql.match(/\(([^)]+)\)\s*VALUES/i);
  if (!match) return -1;
  return match[1].split(",").map((name) => name.trim()).indexOf(column);
}

test("event is dual-written: firehose point + D1 insert", async () => {
  const db = makeDb();
  const firehose = makeFirehose();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db, FIREHOSE: firehose }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, true);
  assert.equal(json.firehose, true);

  assert.equal(firehose.points.length, 1);
  const point = firehose.points[0];
  // index1 = telemetry_id (sampling key)
  assert.deepEqual(point.indexes, ["11111111-2222-3333-4444-555555555555"]);
  // FIREHOSE_SCHEMA blob positions (append-only contract):
  assert.equal(point.blobs[0], "onboarding_step"); // blob1 = event
  assert.equal(point.blobs[7], "auth_failed"); // blob8 = step
  assert.equal(point.blobs[8], "testprov"); // blob9 = auth_provider
  assert.equal(point.blobs[10], "callback_timeout"); // blob11 = auth_failure_reason
  assert.equal(point.blobs.length, 20);
  assert.equal(point.doubles.length, 20);

  assert.ok(db.executed.some(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql)));
});

test("prompt_submitted is accepted and marks meaningful daily activity", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeBody({
      event: "prompt_submitted",
      event_id: "prompt-1",
      session_id: "session-1",
      turn_index: 1,
      build_channel: "release",
    })),
    { DB: db },
    makeCtx(),
  );

  assert.equal(response.status, 200);
  const eventInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(eventInsert, "prompt event should be persisted");
  const rollup = db.executed.find(({ sql }) => /INSERT INTO daily_active_users/.test(sql));
  assert.ok(rollup, "prompt event should update daily activity");
  assert.equal(rollup.values[2], 1, "prompt activity should be meaningful");
  assert.equal(rollup.values[4], 1, "release prompt activity should be meaningful release activity");
});

test("telemetry_opt_out is accepted and persists only coarse metadata", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeBody({
      event: "telemetry_opt_out",
      event_id: "opt-out-1",
      step: "telemetry_settings",
    })),
    { DB: db },
    makeCtx(),
  );

  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert, "opt-out event should be persisted");
  assert.ok(columnIndex(insert.sql, "step") >= 0);
  assert.equal(insert.values[columnIndex(insert.sql, "step")], "telemetry_settings");
  assert.equal(columnIndex(insert.sql, "session_id"), -1);
});

test("session_end persists todo telemetry into session_details", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeBody({
      event: "session_end",
      event_id: "session-end-1",
      session_id: "session-1",
      tool_cat_todo: 4,
      feature_todo_used: true,
      todo_gate_ownership_count: 1,
      todo_gate_hill_count: 2,
      todo_gate_completion_count: 1,
      todo_gate_spike_count: 1,
    })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO session_details/.test(sql));
  assert.ok(detailInsert, "session_details insert should run");
  for (const [column, expected] of [
    ["tool_cat_todo", 4],
    ["feature_todo_used", 1],
    ["todo_gate_ownership_count", 1],
    ["todo_gate_hill_count", 2],
    ["todo_gate_completion_count", 1],
    ["todo_gate_spike_count", 1],
  ]) {
    const idx = columnIndex(detailInsert.sql, column);
    assert.ok(idx >= 0, `${column} should be inserted`);
    assert.equal(detailInsert.values[idx], expected, column);
  }
});

test("turn_end persists todo telemetry into turn_details", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeBody({
      event: "turn_end",
      event_id: "turn-end-1",
      session_id: "session-1",
      turn_index: 2,
      tool_cat_todo: 2,
      feature_todo_used: true,
      todo_gate_hill_count: 1,
    })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO turn_details/.test(sql));
  assert.ok(detailInsert, "turn_details insert should run");
  for (const [column, expected] of [
    ["tool_cat_todo", 2],
    ["feature_todo_used", 1],
    ["todo_gate_hill_count", 1],
    ["todo_gate_ownership_count", 0],
  ]) {
    const idx = columnIndex(detailInsert.sql, column);
    assert.ok(idx >= 0, `${column} should be inserted`);
    assert.equal(detailInsert.values[idx], expected, column);
  }
});

test("discovery event is validated, firehosed, and persisted to details", async () => {
  const db = makeDb();
  const discoveryFirehose = makeFirehose();
  const response = await worker.fetch(
    postRequest(makeDiscoveryBody()),
    { DB: db, FIREHOSE_DISCOVERY: discoveryFirehose },
    makeCtx(),
  );
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.firehose, true);
  assert.equal(discoveryFirehose.points.length, 1);
  const point = discoveryFirehose.points[0];
  assert.equal(point.blobs[7], "11111111-2222-4333-8444-555555555555");
  assert.equal(point.blobs[8], "browse");
  assert.equal(point.blobs[9], "payments");
  assert.equal(point.blobs[11], "success");
  assert.equal(point.doubles[3], 200);
  assert.equal(point.doubles[4], 125);
  assert.equal(point.doubles[7], 1);
  assert.equal(point.doubles[10], 1);

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO discovery_details/.test(sql));
  assert.ok(detailInsert);
  assert.ok(detailInsert.values.includes("11111111-2222-4333-8444-555555555555"));
  assert.ok(detailInsert.values.includes("payments"));
  const detailColumns = detailInsert.sql.match(/\(([^)]+)\)/)[1].split(", ");
  assert.equal(detailInsert.values[detailColumns.indexOf("benchmark_run")], 1);
  assert.ok(!detailInsert.values.some((value) => String(value).includes("virtual card")));
});

test("discovery telemetry accepts the catalog suggest phase", async () => {
  const db = makeDb();
  const discoveryFirehose = makeFirehose();
  const response = await worker.fetch(
    postRequest(makeDiscoveryBody({
      phase: "suggest",
      selected_tool: null,
      http_status: 202,
      result_count: 1,
    })),
    { DB: db, FIREHOSE_DISCOVERY: discoveryFirehose },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  assert.equal(discoveryFirehose.points[0].blobs[8], "suggest");
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO discovery_details/.test(sql));
  const columns = detailInsert.sql.match(/\(([^)]+)\)/)[1].split(", ");
  assert.equal(detailInsert.values[columns.indexOf("phase")], "suggest");
});

test("discovery telemetry accepts and persists the catalog details phase", async () => {
  const db = makeDb();
  const discoveryFirehose = makeFirehose();
  const response = await worker.fetch(
    postRequest(makeDiscoveryBody({
      phase: "details",
      selected_tool: "agentcard",
      result_count: 1,
    })),
    { DB: db, FIREHOSE_DISCOVERY: discoveryFirehose },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  assert.equal(discoveryFirehose.points[0].blobs[8], "details");
  assert.equal(discoveryFirehose.points[0].blobs[10], "agentcard");
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO discovery_details/.test(sql));
  const columns = detailInsert.sql.match(/\(([^)]+)\)/)[1].split(", ");
  assert.equal(detailInsert.values[columns.indexOf("phase")], "details");
  assert.equal(detailInsert.values[columns.indexOf("selected_tool")], "agentcard");
});

test("discovery event rejects unknown failure classifications", async () => {
  const response = await worker.fetch(
    postRequest(makeDiscoveryBody({ outcome: "failure", failure_reason: "raw secret error" })),
    { DB: makeDb(), FIREHOSE_DISCOVERY: makeFirehose() },
    makeCtx(),
  );
  assert.equal(response.status, 400);
  assert.match((await response.json()).error, /failure_reason/);
});

test("todo session event persists numeric aggregates under only its ephemeral correlation id", async () => {
  const db = makeDb();
  const body = makeTodoSessionBody();
  const response = await worker.fetch(postRequest(body), { DB: db }, makeCtx());
  assert.equal(response.status, 200);

  const eventInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(eventInsert);
  const eventColumns = eventInsert.sql.match(/\(([^)]+)\)/)[1].split(", ");
  assert.equal(eventInsert.values[eventColumns.indexOf("telemetry_id")], body.correlation_id);

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO todo_session_details/.test(sql));
  assert.ok(detailInsert);
  const columns = detailInsert.sql.match(/\(([^)]+)\)/)[1].split(", ");
  assert.equal(detailInsert.values[columns.indexOf("correlation_id")], body.correlation_id);
  assert.equal(detailInsert.values[columns.indexOf("todos_completed")], 3);
  assert.equal(detailInsert.values[columns.indexOf("confidence_mean")], 82.5);
  assert.ok(!columns.includes("content"));
  assert.ok(!columns.includes("feedback_loop"));
});

test("todo session event rejects a persistent id distinct from its correlation id", async () => {
  const response = await worker.fetch(
    postRequest(makeTodoSessionBody({ id: "11111111-2222-4333-8444-555555555555" })),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.status, 400);
  assert.match((await response.json()).error, /must equal correlation_id/);
});

test("D1 failure with firehose success degrades to durable:false instead of 500", async () => {
  const db = makeDb({ failInserts: true });
  const firehose = makeFirehose();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db, FIREHOSE: firehose }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, false);
  assert.equal(json.firehose, true);
  assert.equal(firehose.points.length, 1);
});

test("SQLITE_FULL-class insert failure schedules an emergency prune", async () => {
  const db = makeDb({ failInserts: true, failureMessage: "SQLITE_FULL: database or disk is full" });
  const firehose = makeFirehose();
  const ctx = makeCtx();

  await worker.fetch(postRequest(makeBody()), { DB: db, FIREHOSE: firehose }, ctx);
  // The prune is scheduled via ctx.waitUntil; drain it and check DELETEs ran.
  await Promise.all(ctx.waited);

  assert.ok(
    db.executed.some(({ sql }) => /DELETE FROM events/.test(sql)),
    "emergency prune should issue DELETEs after a full-database failure",
  );
});

test("D1 failure without firehose binding still returns 500", async () => {
  const db = makeDb({ failInserts: true, failureMessage: "some transient error" });
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db }, ctx);
  assert.equal(response.status, 500);
});

test("missing firehose binding degrades gracefully", async () => {
  const db = makeDb();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeBody()), { DB: db }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, true);
  assert.equal(json.firehose, false);
});

test("health endpoint reports database size vs soft limit", async () => {
  const db = makeDb({ sizeAfter: 12345678 });
  const ctx = makeCtx();

  const response = await worker.fetch(new Request(HEALTH_URL, { method: "GET" }), { DB: db }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.db_size_bytes, 12345678);
  assert.equal(json.db_soft_limit_bytes, 4_500_000_000);
  assert.equal(json.over_soft_limit, false);
});

test("paid-plan database size below the budget guardrail is healthy", async () => {
  const db = makeDb({ sizeAfter: 1_200_000_000 });
  const response = await worker.fetch(
    new Request(HEALTH_URL, { method: "GET" }),
    { DB: db },
    makeCtx(),
  );
  const json = await response.json();

  assert.equal(json.db_size_bytes, 1_200_000_000);
  assert.equal(json.over_soft_limit, false);
});

test("database size above the paid-plan budget guardrail is reported", async () => {
  const db = makeDb({ sizeAfter: 4_600_000_000 });
  const response = await worker.fetch(
    new Request(HEALTH_URL, { method: "GET" }),
    { DB: db },
    makeCtx(),
  );
  const json = await response.json();

  assert.equal(json.db_size_bytes, 4_600_000_000);
  assert.equal(json.over_soft_limit, true);
});

test("unknown event type is rejected", async () => {
  const db = makeDb();
  const ctx = makeCtx();
  const response = await worker.fetch(
    postRequest(makeBody({ event: "mystery" })),
    { DB: db },
    ctx,
  );
  assert.equal(response.status, 400);
});

// ---------------------------------------------------------------------------
// Website analytics events (web_pageview / web_cta_click)
// ---------------------------------------------------------------------------

function makeWebBody(overrides = {}) {
  return {
    event: "web_pageview",
    visitor_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    path: "/pricing",
    referrer: "https://news.ycombinator.com/",
    utm_source: "hn",
    utm_medium: "social",
    utm_campaign: "launch",
    event_id: "web-event-1",
    session_id: "web-session-1",
    pageview_id: "web-pageview-1",
    ...overrides,
  };
}

test("web_pageview is normalized and stored in events + web_details", async () => {
  const db = makeDb();
  const ctx = makeCtx();

  const response = await worker.fetch(postRequest(makeWebBody()), { DB: db }, ctx);
  const json = await response.json();

  assert.equal(response.status, 200);
  assert.equal(json.ok, true);
  assert.equal(json.durable, true);

  const eventsInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(eventsInsert, "events row inserted");
  // visitor_id doubles as the telemetry id; version/os/arch are defaulted.
  assert.ok(eventsInsert.values.includes("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
  assert.ok(eventsInsert.values.includes("web"));

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert, "web_details row inserted");
  assert.ok(detailInsert.values.includes("/pricing"));
  assert.ok(detailInsert.values.includes("hn"));
});

test("web_pageview without event_id mints one so web_details still lands", async () => {
  // Defensive compatibility for older beacons and hand-written clients.
  const db = makeDb();
  const ctx = makeCtx();

  const body = makeWebBody();
  delete body.event_id;
  const response = await worker.fetch(postRequest(body), { DB: db }, ctx);
  assert.equal(response.status, 200);

  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert, "web_details row inserted despite missing event_id");
  assert.ok(detailInsert.values.includes("/pricing"));
});

test("web_pageview without visitor_id is rejected", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeWebBody({ visitor_id: undefined })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 400);
});

test("web_pageview without path is rejected", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeWebBody({ path: undefined })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 400);
});

test("web_cta_click requires cta", async () => {
  const db = makeDb();
  const missing = await worker.fetch(
    postRequest(makeWebBody({ event: "web_cta_click" })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(missing.status, 400);

  const ok = await worker.fetch(
    postRequest(makeWebBody({ event: "web_cta_click", cta: "plus_early_access" })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(ok.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert.values.includes("plus_early_access"));
});

test("install CTA details retain the anonymous conversion dimensions", async () => {
  const db = makeDb();
  const webFirehose = makeFirehose();
  const installFirehose = makeFirehose();
  const conversionId = "11111111-2222-4333-8444-555555555555";
  const response = await worker.fetch(
    postRequest(makeWebBody({
      event: "web_cta_click",
      cta: "install",
      conversion_id: conversionId,
      placement: "hero",
      install_method: "shell",
    })),
    { DB: db, FIREHOSE_WEB: webFirehose, FIREHOSE_INSTALL: installFirehose },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert.values.includes(conversionId));
  assert.ok(detailInsert.values.includes("web-pageview-1"));
  assert.ok(detailInsert.values.includes("hero"));
  assert.ok(detailInsert.values.includes("shell"));
  assert.equal(webFirehose.points.length, 1);
  assert.equal(installFirehose.points.length, 1);
  assert.deepEqual(installFirehose.points[0].indexes, [conversionId]);
  assert.equal(installFirehose.points[0].blobs[0], "web_cta_click");
  assert.equal(installFirehose.points[0].blobs[2], conversionId);
  assert.equal(installFirehose.points[0].blobs[6], "hero");
});

function makeInstallFunnelBody(overrides = {}) {
  return {
    id: "11111111-2222-4333-8444-555555555555",
    event: "install_funnel",
    version: "web",
    os: "web",
    arch: "web",
    conversion_id: "11111111-2222-4333-8444-555555555555",
    stage: "script_request",
    outcome: "success",
    source: "install_endpoint",
    install_method: "shell",
    ...overrides,
  };
}

test("install funnel stages are validated and persisted in install_details", async () => {
  const db = makeDb();
  const installFirehose = makeFirehose();
  const response = await worker.fetch(
    postRequest(makeInstallFunnelBody()),
    { DB: db, FIREHOSE_INSTALL: installFirehose },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO install_details/.test(sql));
  assert.ok(detailInsert, "install_details row inserted");
  assert.ok(detailInsert.values.includes("script_request"));
  assert.ok(detailInsert.values.includes("success"));
  assert.equal(installFirehose.points.length, 1);
  assert.equal(installFirehose.points[0].blobs[3], "script_request");
  assert.equal(installFirehose.points[0].blobs[4], "success");

  const invalid = await worker.fetch(
    postRequest(makeInstallFunnelBody({ conversion_id: "not-a-uuid" })),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(invalid.status, 400);
});

test("first-run install events join to the conversion id without widening events", async () => {
  const db = makeDb();
  const conversionId = "11111111-2222-4333-8444-555555555555";
  const response = await worker.fetch(
    postRequest(makeBody({
      event: "install",
      event_id: "install-event-1",
      step: undefined,
      install_conversion_id: conversionId,
    })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const eventInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.equal(eventInsert.sql.includes("conversion_id"), false);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO install_details/.test(sql));
  assert.ok(detailInsert.values.includes(conversionId));
  assert.ok(detailInsert.values.includes("first_run"));
  assert.ok(detailInsert.values.includes("cli"));
});

test("web free-text fields are length-capped (size defense)", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeWebBody({ path: "/" + "x".repeat(5000), referrer: "r".repeat(5000) })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  for (const value of detailInsert.values) {
    assert.ok(String(value).length <= 200, "web detail values capped at 200 chars");
  }
});

test("web events are firehosed to FIREHOSE_WEB with visitor_id as index1", async () => {
  const db = makeDb();
  const firehose = makeFirehose();
  const webFirehose = makeFirehose();

  const response = await worker.fetch(
    postRequest(makeWebBody({ event: "web_cta_click", cta: "install" })),
    { DB: db, FIREHOSE: firehose, FIREHOSE_WEB: webFirehose },
    makeCtx(),
  );
  const json = await response.json();

  assert.equal(json.firehose, true);
  assert.equal(firehose.points.length, 0, "CLI firehose untouched by web events");
  assert.equal(webFirehose.points.length, 1);
  const point = webFirehose.points[0];
  assert.deepEqual(point.indexes, ["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]);
  // FIREHOSE_WEB_SCHEMA blob positions (append-only contract):
  assert.equal(point.blobs[0], "web_cta_click"); // blob1 = event
  assert.equal(point.blobs[7], "/pricing"); // blob8 = path
  assert.equal(point.blobs[9], "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"); // blob10 = visitor_id
  assert.equal(point.blobs[13], "install"); // blob14 = cta
});

test("web_vital validates, caps, stores, and appends firehose fields", async () => {
  const db = makeDb();
  const webFirehose = makeFirehose();
  const response = await worker.fetch(
    postRequest(makeWebBody({
      event: "web_vital",
      metric_name: "LCP",
      metric_value: 999_999,
      rating: "poor",
      message: "must not persist",
      url: "https://jcode.sh/private?token=secret",
    })),
    { DB: db, FIREHOSE_WEB: webFirehose },
    makeCtx(),
  );

  assert.equal(response.status, 200);
  const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(detailInsert.sql.includes("metric_name"));
  assert.ok(detailInsert.sql.includes("metric_value"));
  assert.ok(detailInsert.sql.includes("rating"));
  assert.ok(detailInsert.values.includes("LCP"));
  assert.ok(detailInsert.values.includes(300_000));
  assert.ok(detailInsert.values.includes("poor"));
  assert.ok(!detailInsert.values.some((value) => String(value).includes("must not persist")));
  assert.ok(!detailInsert.values.some((value) => String(value).includes("token=secret")));

  const point = webFirehose.points[0];
  assert.equal(point.blobs[17], "LCP"); // blob18 = metric_name
  assert.equal(point.blobs[18], "poor"); // blob19 = rating
  assert.equal(point.blobs[19], ""); // blob20 = error_kind
  assert.equal(point.doubles[1], 300_000); // double2 = metric_value
});

test("web_vital accepts only standard finite nonnegative metrics and ratings", async () => {
  const invalidBodies = [
    { metric_name: "FID", metric_value: 1, rating: "good" },
    { metric_name: "CLS", metric_value: -1, rating: "poor" },
    { metric_name: "CLS", metric_value: "0.1", rating: "good" },
    { metric_name: "CLS", metric_value: null, rating: "good" },
    { metric_name: "CLS", metric_value: 0.1, rating: "okay" },
  ];
  for (const fields of invalidBodies) {
    const response = await worker.fetch(
      postRequest(makeWebBody({ event: "web_vital", ...fields })),
      { DB: makeDb(), FIREHOSE_WEB: makeFirehose() },
      makeCtx(),
    );
    assert.equal(response.status, 400, JSON.stringify(fields));
  }

  const clsDb = makeDb();
  const clsResponse = await worker.fetch(
    postRequest(makeWebBody({ event: "web_vital", metric_name: "CLS", metric_value: 99, rating: "poor" })),
    { DB: clsDb },
    makeCtx(),
  );
  assert.equal(clsResponse.status, 200);
  const clsInsert = clsDb.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
  assert.ok(clsInsert.values.includes(10));
});

test("web_error stores only an allowed coarse classification", async () => {
  for (const error_kind of ["script", "promise", "resource"]) {
    const db = makeDb();
    const webFirehose = makeFirehose();
    const response = await worker.fetch(
      postRequest(makeWebBody({
        event: "web_error",
        error_kind,
        error_message: "private failure detail",
        stack: "secret stack",
        filename: "https://cdn.example/private.js",
      })),
      { DB: db, FIREHOSE_WEB: webFirehose },
      makeCtx(),
    );
    assert.equal(response.status, 200);
    const detailInsert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO web_details/.test(sql));
    assert.ok(detailInsert.values.includes(error_kind));
    assert.ok(!detailInsert.values.some((value) => /private|secret|cdn\.example|ycombinator/.test(String(value))));
    assert.equal(webFirehose.points[0].blobs[19], error_kind); // blob20
  }

  const rejected = await worker.fetch(
    postRequest(makeWebBody({ event: "web_error", error_kind: "TypeError: secret" })),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(rejected.status, 400);
});

test("scheduled retention prunes funnel events and redacts conversion joins after 90 days", async () => {
  const db = makeDb();
  const ctx = makeCtx();
  await worker.scheduled({}, { DB: db }, ctx);
  await Promise.all(ctx.waited);

  const eventDeletes = db.executed.filter(({ sql }) => /DELETE FROM events WHERE id IN/.test(sql));
  assert.ok(eventDeletes.some(({ values }) => values[0] === "web_vital" && values[1] === "-30 days"));
  assert.ok(eventDeletes.some(({ values }) => values[0] === "web_error" && values[1] === "-90 days"));
  assert.ok(eventDeletes.some(({ values }) => values[0] === "install_funnel" && values[1] === "-90 days"));
  const redactions = db.executed.filter(({ sql }) => /UPDATE (web_details|install_details) SET conversion_id = NULL/.test(sql));
  assert.equal(redactions.length, 2);
  assert.ok(redactions.every(({ values }) => values[0] === "-90 days"));
  assert.ok(db.executed.some(({ sql, values }) =>
    /DELETE FROM install_details WHERE event_id IN/.test(sql) && values[0] === "install_funnel"
  ));
});

// ---------------------------------------------------------------------------
// Token subscription plan events
// ---------------------------------------------------------------------------

function makeSubscriptionBody(overrides = {}) {
  return makeBody({
    event: "subscription_activated",
    step: undefined,
    auth_provider: undefined,
    auth_method: undefined,
    auth_failure_reason: undefined,
    account_id: "acct_123",
    tier: "plus",
    ...overrides,
  });
}

test("subscription events require account_id", async () => {
  const db = makeDb();
  for (const event of [
    "subscription_login",
    "subscription_activated",
    "subscription_budget_exhausted",
    "subscription_router_error",
    "account_linked",
  ]) {
    const response = await worker.fetch(
      postRequest(makeSubscriptionBody({ event, account_id: undefined })),
      { DB: db },
      makeCtx(),
    );
    assert.equal(response.status, 400, `${event} without account_id rejected`);
  }
});

test("subscription_activated stores account_id and tier", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeSubscriptionBody()),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert.sql.includes("account_id"));
  assert.ok(insert.sql.includes("tier"));
  assert.ok(insert.values.includes("acct_123"));
  assert.ok(insert.values.includes("plus"));
});

test("subscription model is stored in the generic model_start column", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeSubscriptionBody({ event: "subscription_router_error", model: "gpt-5.5" })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert.sql.includes("model_start"));
  assert.ok(insert.values.includes("gpt-5.5"));
});

test("account_linked joins telemetry_id and account_id", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeSubscriptionBody({ event: "account_linked", tier: undefined })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  const insert = db.executed.find(({ sql }) => /INSERT OR IGNORE INTO events/.test(sql));
  assert.ok(insert.values.includes("11111111-2222-3333-4444-555555555555"));
  assert.ok(insert.values.includes("acct_123"));
});

// ---------------------------------------------------------------------------
// CORS for the website beacon
// ---------------------------------------------------------------------------

test("OPTIONS preflight from jcode.sh echoes the origin", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://jcode.sh" },
    }),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://jcode.sh");
  assert.equal(response.headers.get("Vary"), "Origin");
  assert.ok(/POST/.test(response.headers.get("Access-Control-Allow-Methods")));
});

test("OPTIONS preflight from the production website echoes the origin", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://solosystems.dev" },
    }),
    { DB: makeDb(), ALLOWED_ORIGIN: "https://fallback.example" },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://solosystems.dev");
  assert.equal(response.headers.get("Vary"), "Origin");
});

test("OPTIONS preflight from pages.dev preview echoes the origin", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://solosystems.pages.dev" },
    }),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://solosystems.pages.dev");
});

test("other origins fall back to ALLOWED_ORIGIN default", async () => {
  const response = await worker.fetch(
    new Request(EVENT_URL, {
      method: "OPTIONS",
      headers: { Origin: "https://evil.example" },
    }),
    { DB: makeDb() },
    makeCtx(),
  );
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "*");
});

test("POST responses from the beacon origin carry CORS headers", async () => {
  const db = makeDb();
  const request = new Request(EVENT_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Origin: "https://jcode.sh",
    },
    body: JSON.stringify(makeWebBody()),
  });
  const response = await worker.fetch(request, { DB: db }, makeCtx());
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://jcode.sh");
});

// ---------------------------------------------------------------------------
// Coarse geography (country only, resolved at Cloudflare's edge).
// ---------------------------------------------------------------------------

function postRequestFromCountry(body, country, url = EVENT_URL) {
  const request = postRequest(body, url);
  Object.defineProperty(request, "cf", { value: { country }, configurable: true });
  return request;
}

test("country is taken from request.cf and rolled up per day", async () => {
  const db = makeDb();
  const geo = makeFirehose();
  const response = await worker.fetch(
    postRequestFromCountry(makeBody({ event: "install" }), "de"),
    { DB: db, FIREHOSE_GEO: geo },
    makeCtx(),
  );
  assert.equal(response.status, 200);

  // Geo firehose point: blob2 = country, normalized to uppercase.
  assert.equal(geo.points.length, 1);
  assert.equal(geo.points[0].blobs[0], "install");
  assert.equal(geo.points[0].blobs[1], "DE");

  const rollup = db.executed.find(({ sql }) => /INSERT INTO country_daily/.test(sql));
  assert.ok(rollup, "country_daily rollup should be written");
  assert.equal(rollup.values[1], "DE");
  assert.equal(rollup.values[2], "install");
  assert.equal(rollup.values[3], 0);
});

test("lifecycle events stamp last_country on the DAU rollup", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequestFromCountry(makeBody({ event: "session_end", event_id: "se-geo" }), "JP"),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);

  const dau = db.executed.find(({ sql }) => /INSERT INTO daily_active_users/.test(sql));
  assert.ok(dau, "daily_active_users rollup should be written");
  assert.ok(columnIndex(dau.sql, "last_country") >= 0, "last_country column should be present");
  // last_country is the final bound placeholder (raw_active is a literal 1, so
  // column positions and bind positions are intentionally not aligned).
  assert.equal(dau.values[dau.values.length - 1], "JP");
});

test("CI-built artifacts count as releases without becoming runtime CI", async () => {
  const db = makeDb();
  const response = await worker.fetch(
    postRequest(makeBody({
      event: "session_end",
      event_id: "se-ci-built-release",
      build_channel: "ci_release",
      is_ci: false,
      turns: 1,
    })),
    { DB: db },
    makeCtx(),
  );
  assert.equal(response.status, 200);

  const dau = db.executed.find(({ sql }) => /INSERT INTO daily_active_users/.test(sql));
  assert.ok(dau, "daily_active_users rollup should be written");
  assert.equal(dau.values[3], 1, "CI-built binary remains release activity");
  assert.equal(dau.values[4], 1, "meaningful CI-built work remains release activity");
  assert.equal(dau.values[9], 0, "build provenance must not imply runtime CI");
  assert.equal(dau.values[10], 0, "last runtime CI flag remains false");
  assert.equal(dau.values[11], "ci_release");
});

test("client-supplied country is ignored and bogus codes are dropped", async () => {
  const db = makeDb();
  const geo = makeFirehose();
  // "XX" (unknown) and "T1" (Tor) are not real countries; a spoofed body field
  // must never win over the edge value.
  const response = await worker.fetch(
    postRequestFromCountry(makeBody({ event: "install", country: "US" }), "XX"),
    { DB: db, FIREHOSE_GEO: geo },
    makeCtx(),
  );
  assert.equal(response.status, 200);
  assert.equal(geo.points.length, 0);
  assert.ok(!db.executed.some(({ sql }) => /INSERT INTO country_daily/.test(sql)));
});

test("missing geo binding and missing cf never break the event insert", async () => {
  const db = makeDb();
  const response = await worker.fetch(postRequest(makeBody()), { DB: db }, makeCtx());
  const json = await response.json();
  assert.equal(response.status, 200);
  assert.equal(json.durable, true);
});
