import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import worker, { normalizeUsageReportEvent, FIREHOSE_USAGE_SCHEMA } from "../src/worker.js";
const sql = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
function database() { const db = new DatabaseSync(":memory:"); db.exec(sql("schema.sql")); return db; }
function body(extra = {}) { return { id: "install-a", event: "usage_report", event_id: "response-a", session_id: "session-a", version: "test", os: "linux", arch: "x64", source: "agent", provider: "OpenAI", model: "test-model", input_tokens: 100, output_tokens: 20, cache_read_input_tokens: 80, cache_creation_input_tokens: 5, total_tokens: 205, is_ci: false, ...extra }; }
function d1(db) { return { prepare(statement) { const execute = (values) => ({ async run() { const r = db.prepare(statement).run(...values.map((value) => value instanceof ArrayBuffer ? new Uint8Array(value) : value)); return { meta: { changes: Number(r.changes) } }; }, async all() { return { results: db.prepare(statement).all(...values) }; } }); return { ...execute([]), bind: (...values) => execute(values) }; } }; }
async function ingest(db, value, extra = {}, path = "/v1/event") { const response = await worker.fetch(new Request(`https://example.test${path}`, { method: "POST", body: JSON.stringify(value) }), { DB: d1(db), ...extra }, {}); return { status: response.status, ...await response.json() }; }
const usage = (db) => db.prepare("SELECT * FROM daily_model_usage").all();
test("response model drives rollup while coarse attribution and firehose retain alias", async () => {
  const db = database();
  const points = [];
  const value = body({ usage_contract_version: 2, occurred_at: new Date().toISOString(),
    model: "high", requested_model: "high", resolved_model: "vendor/future-model-2030" });
  for (let n = 0; n < 2; n++) assert.equal((await ingest(db, value, {
    FIREHOSE_USAGE: { writeDataPoint(point) { points.push(point); } },
  }, "/v2/usage")).durable, true);
  const row = db.prepare("SELECT * FROM daily_model_usage_v2").get();
  assert.equal(row.model, "vendor/future-model-2030");
  assert.equal(row.responses, 1);
  const receipt = db.prepare("SELECT * FROM usage_report_ledger").get();
  assert.equal(receipt.requested_model, null);
  assert.equal(receipt.resolved_model, null);
  assert.equal(receipt.model, null, "transient rollup payload remains compacted");
  const attribution = db.prepare("SELECT * FROM daily_model_attribution_v1").get();
  assert.equal(attribution.requested_model, "high");
  assert.equal(attribution.resolved_model, row.model);
  assert.equal(attribution.responses, 1, "retry must not double count attribution");
  assert.equal(attribution.total_tokens, row.total_tokens);
  const blob = (key) => points[0].blobs[FIREHOSE_USAGE_SCHEMA.blobs.indexOf(key)];
  assert.equal(blob("requested_model"), "high");
  assert.equal(blob("resolved_model"), row.model);
  assert.equal(blob("model"), row.model);
  assert.equal(db.prepare("SELECT count(*) n FROM events").get().n, 0);
});

test("absent resolution remains unknown and metadata-free clients stay compatible", async () => {
  const db = database();
  await ingest(db, body({ model: "old-alias" }));
  let receipt = db.prepare("SELECT * FROM usage_report_ledger").get();
  assert.equal(receipt.requested_model, null);
  assert.equal(receipt.resolved_model, null);
  assert.equal(db.prepare("SELECT count(*) n FROM daily_model_attribution_v1").get().n, 0);
  assert.equal(usage(db)[0].model, "old-alias");
  await ingest(db, body({ event_id: "new", model: "alias", requested_model: "alias" }));
  const attribution = db.prepare("SELECT * FROM daily_model_attribution_v1").get();
  assert.equal(attribution.requested_model, "alias");
  assert.equal(attribution.resolved_model, "");
  assert.equal(db.prepare("SELECT count(*) n FROM usage_report_ledger WHERE requested_model IS NOT NULL OR resolved_model IS NOT NULL").get().n, 0);
  assert.deepEqual(usage(db).map((row) => row.model).sort(), ["alias", "old-alias"]);
});

test("distinct receipts aggregate while aliases and contract versions remain separate", async () => {
  const db = database();
  const value = body({ usage_contract_version: 2, occurred_at: new Date().toISOString(),
    model: "actual", requested_model: "high", resolved_model: "actual" });
  for (const [event_id, requested_model] of [["one", "high"], ["two", "high"], ["three", "medium"]]) {
    assert.equal((await ingest(db, { ...value, event_id, requested_model })).durable, true);
  }
  assert.equal(db.prepare("SELECT responses FROM daily_model_usage_v2").get().responses, 3);
  const rows = db.prepare("SELECT * FROM daily_model_attribution_v1 ORDER BY requested_model").all();
  assert.deepEqual(rows.map((row) => [row.requested_model, row.responses, row.total_tokens]),
    [["high", 2, 410], ["medium", 1, 205]]);
  assert.equal((await ingest(db, { ...value, event_id: "native", usage_contract_version: 1 })).durable, true);
  assert.equal(db.prepare("SELECT responses FROM daily_model_attribution_v1 WHERE usage_contract_version=1").get().responses, 1);
  assert.equal(db.prepare("SELECT responses FROM daily_model_usage").get().responses, 1);
});

test("attribution names are bounded and normalization uses effective response ID", async () => {
  for (const field of ["requested_model", "resolved_model"]) {
    for (const value of [42, {}, [], "", "  ", "a".repeat(121), "界".repeat(41), "bad\nmodel", "bad\u001bmodel"]) {
      const db = database();
      assert.equal((await ingest(db, body({ [field]: value }))).status, 400, `${field}: ${JSON.stringify(value)}`);
      assert.equal(db.prepare("SELECT count(*) n FROM usage_report_ledger").get().n, 0);
    }
  }
  const value = body({ model: "alias", resolved_model: "  vendor/model  " });
  assert.equal(normalizeUsageReportEvent(value), null);
  assert.equal(value.requested_model, "alias");
  assert.equal(value.resolved_model, "vendor/model");
  assert.equal(value.model, "vendor/model");
});

test("attribution migration preserves historical rollups and compact receipts", async () => {
  const db = database();
  db.exec("DROP TRIGGER usage_report_rollup; DROP TRIGGER usage_report_rollup_v2; DROP TABLE usage_report_ledger; DROP TABLE daily_model_attribution_v1");
  db.exec(sql("migrations/0031_usage_report_receipts.sql"));
  // Populate the pre-migration schema through its real atomic rollup trigger.
  db.exec(`INSERT INTO usage_report_ledger (receipt_key, usage_date, source, provider, model,
    build_channel, is_ci, responses, input_tokens, output_tokens, cache_read_input_tokens,
    cache_creation_input_tokens, total_tokens) VALUES (X'01', '2026-01-01', 'agent',
    'OpenAI', 'historical-alias', '', 0, 1, 10, 2, 0, 0, 12)`);
  db.exec(sql("migrations/0032_usage_model_attribution.sql"));
  assert.equal(usage(db)[0].model, "historical-alias");
  assert.equal(db.prepare("SELECT resolved_model FROM usage_report_ledger").get().resolved_model, null);
  assert.equal(db.prepare("SELECT count(*) n FROM daily_model_attribution_v1").get().n, 0);
  assert.equal((await ingest(db, body({ model: "alias", requested_model: "alias", resolved_model: "actual-model" }))).durable, true);
  assert.deepEqual(usage(db).map((row) => row.model).sort(), ["actual-model", "historical-alias"]);
  assert.equal(db.prepare("SELECT requested_model FROM daily_model_attribution_v1").get().requested_model, "alias");
  const fresh = database();
  for (const name of ["usage_report_rollup", "usage_report_rollup_v2"]) {
    const trigger = (connection) => connection.prepare("SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?").get(name).sql;
    assert.equal(trigger(db), trigger(fresh), "migration and fresh schema must have identical atomic trigger bodies");
  }
});

test("attribution rollup failure rolls back canonical usage and receipt for retry", async () => {
  const db = database();
  const value = body({ usage_contract_version: 2, occurred_at: new Date().toISOString(),
    model: "actual", requested_model: "alias", resolved_model: "actual" });
  db.exec("CREATE TRIGGER fail_attribution BEFORE INSERT ON daily_model_attribution_v1 BEGIN SELECT RAISE(ABORT, 'injected'); END");
  assert.equal((await ingest(db, value)).status, 503);
  for (const table of ["usage_report_ledger", "daily_model_usage_v2", "daily_model_attribution_v1"]) {
    assert.equal(db.prepare(`SELECT count(*) n FROM ${table}`).get().n, 0);
  }
  db.exec("DROP TRIGGER fail_attribution");
  for (let n = 0; n < 3; n++) assert.equal((await ingest(db, value)).durable, true);
  assert.equal(db.prepare("SELECT responses FROM daily_model_attribution_v1").get().responses, 1);
  assert.equal(db.prepare("SELECT responses FROM daily_model_usage_v2").get().responses, 1);
});

test("v2 usage rejects incompatible and malformed bodies before any storage access", async () => {
  const db = database();
  const v2 = body({ usage_contract_version: 2, occurred_at: new Date().toISOString() });
  const snapshot = () => db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'").all()
    .map(({ name }) => [name, db.prepare(`SELECT * FROM "${name}"`).all()]);
  const before = snapshot();
  const extra = {
    DB: { prepare() { assert.fail("invalid body must not access D1"); } },
    FIREHOSE_USAGE: { writeDataPoint() { assert.fail("invalid body must not reach firehose"); } },
    FIREHOSE: { writeDataPoint() { assert.fail("invalid body must not reach legacy firehose"); } },
  };
  for (const value of [null, [], "usage_report", 2, true,
    body(), { ...v2, usage_contract_version: 1 }, { ...v2, usage_contract_version: "2" },
    { ...v2, usage_contract_version: 3 }, { ...v2, event: "session_start" },
    { ...v2, event: "web_pageview" }, { ...v2, id: "" },
    { ...v2, occurred_at: undefined }, { ...v2, total_tokens: 120 },
    { ...v2, occurred_at: new Date(Date.now() - 8 * 86400000).toISOString() },
    { ...v2, cache_read_input_tokens: -1 }]) {
    assert.equal((await ingest(db, value, extra, "/v2/usage")).status, 400, JSON.stringify(value));
  }
  const response = await worker.fetch(new Request("https://example.test/v2/usage", {
    method: "POST", body: "{",
  }), extra, {});
  assert.equal(response.status, 400);
  assert.deepEqual(snapshot(), before);
});

test("v2 usage preserves occurrence day, compact receipts and durable retry failures", async () => {
  const db = database();
  const occurred_at = new Date(Date.now() - 2 * 86400000).toISOString();
  const value = body({ usage_contract_version: 2, occurred_at });
  db.exec("CREATE TRIGGER fail_v2 BEFORE INSERT ON daily_model_usage_v2 BEGIN SELECT RAISE(ABORT, 'injected'); END");
  assert.equal((await ingest(db, value, {}, "/v2/usage")).status, 503);
  assert.equal(db.prepare("SELECT count(*) n FROM usage_report_ledger").get().n, 0);
  db.exec("DROP TRIGGER fail_v2");
  assert.equal((await ingest(db, value, {}, "/v2/usage")).durable, true);
  assert.equal(db.prepare("SELECT usage_date FROM daily_model_usage_v2").get().usage_date, occurred_at.slice(0, 10));
  assert.equal(db.prepare("SELECT model FROM usage_report_ledger").get().model, null);
});

test("duplicate retries are atomic and scoped by install ID", async () => {
  const db = database();
  for (let n = 0; n < 5; n++) assert.equal((await ingest(db, body())).durable, true);
  assert.equal(usage(db)[0].responses, 1);
  assert.equal(usage(db)[0].total_tokens, 205);
  await ingest(db, body({ id: "install-b" }));
  assert.equal(usage(db)[0].responses, 2);
  assert.equal(db.prepare("SELECT count(*) n FROM events").get().n, 0);
  const receipt = db.prepare("SELECT * FROM usage_report_ledger LIMIT 1").get();
  assert.equal(receipt.model, null, "payload compacted after atomic rollup");
});
test("rollup failure rolls back receipt so retry can recover", async () => {
  const db = database();
  db.exec("CREATE TRIGGER fail_usage BEFORE INSERT ON daily_model_usage BEGIN SELECT RAISE(ABORT, 'injected'); END");
  assert.equal((await ingest(db, body())).status, 503);
  assert.equal(db.prepare("SELECT count(*) n FROM usage_report_ledger").get().n, 0);
  db.exec("DROP TRIGGER fail_usage");
  assert.equal((await ingest(db, body())).status, 200);
  assert.equal(usage(db)[0].responses, 1);
});
test("missing migration cannot ACK even when complete usage backup is written", async () => {
  const db = database(); db.exec("DROP TABLE usage_report_ledger");
  const points = [];
  const result = await ingest(db, body(), { FIREHOSE_USAGE: { writeDataPoint(point) { points.push(point); } }, FIREHOSE: { writeDataPoint() { assert.fail("old incomplete sink must not ACK usage"); } } });
  assert.equal(result.status, 503); assert.equal(result.durable, false); assert.equal(result.firehose, true);
  assert.deepEqual(points[0].doubles.slice(0, 6), [100, 20, 80, 5, 205, 1]);
  assert.equal(points[0].blobs[FIREHOSE_USAGE_SCHEMA.blobs.indexOf("event_id")], "response-a");
});
test("occurrence date survives offline replay and stale/future timestamps are rejected", async () => {
  const db = database(); const occurred_at = new Date(Date.now() - 2 * 86400000).toISOString();
  await ingest(db, body({ occurred_at }));
  assert.equal(usage(db)[0].usage_date, occurred_at.slice(0, 10));
  for (const stamp of ["garbage", "2026-02-30T01:00:00Z", new Date(Date.now() - 9 * 86400000).toISOString(), new Date(Date.now() + 3600000).toISOString()]) {
    assert.equal((await ingest(db, body({ event_id: stamp, occurred_at: stamp }))).status, 400);
  }
});
test("receipt migration is repeatable and fresh schema includes daily rollup", () => {
  const db = database(); db.exec(sql("migrations/0031_usage_report_receipts.sql")); db.exec(sql("migrations/0031_usage_report_receipts.sql"));
  assert.equal(db.prepare("SELECT count(*) n FROM daily_model_usage").get().n, 0);
});
test("strict usage validation rejects malformed IDs, CI flags, counts and overflow", () => {
  for (const extra of [{ event_id: "" }, { event_id: 7 }, { is_ci: "false" }, { input_tokens: "100" }, { input_tokens: NaN }, { responses: 10001 }, { cache_read_input_tokens: -1 }]) assert.ok(normalizeUsageReportEvent(body(extra)), JSON.stringify(extra));
});

test("canonical v2 is isolated from native legacy buckets and identity ACK is explicit", async () => {
  const db = database();
  const v2 = body({ usage_contract_version: 2, occurred_at: new Date().toISOString() });
  const ack = await ingest(db, v2, {}, "/v2/usage");
  assert.equal(ack.status, 200);
  assert.equal(ack.id, v2.id);
  assert.equal(ack.event_id, v2.event_id);
  await Promise.all(Array.from({ length: 10 }, () => ingest(db, v2, {}, "/v2/usage")));
  assert.equal(usage(db).length, 0);
  const canonical = db.prepare("SELECT * FROM daily_model_usage_v2").get();
  assert.equal(canonical.responses, 1);
  assert.equal(canonical.input_tokens, 100);
  assert.equal(canonical.total_tokens, 205);
  await ingest(db, body({ event_id: "native", total_tokens: 120 }));
  assert.equal(usage(db)[0].total_tokens, 120, "native semantics not rewritten");
  assert.equal((await ingest(db, { ...v2, event_id: "bad-total", total_tokens: 120 })).status, 400);
  assert.equal((await ingest(db, { ...v2, event_id: "missing-date", occurred_at: undefined })).status, 400);
  const normalized = { ...v2, total_tokens: undefined };
  assert.equal(normalizeUsageReportEvent(normalized), null);
  assert.equal(normalized.total_tokens, 205);
});

test("expiry cleanup preserves rollup and old timestamp cannot be counted after receipt GC", async () => {
  const db = database();
  const v2 = body({ usage_contract_version: 2, occurred_at: new Date().toISOString() });
  await ingest(db, v2);
  db.exec("UPDATE usage_report_ledger SET created_at=unixepoch('now','-9 days')");
  const jobs = [];
  await worker.scheduled({ cron: "17 4 * * *" }, { DB: d1(db) }, { waitUntil(promise) { jobs.push(promise); } });
  await Promise.all(jobs);
  assert.equal(db.prepare("SELECT count(*) n FROM usage_report_ledger").get().n, 0);
  assert.equal(db.prepare("SELECT responses FROM daily_model_usage_v2").get().responses, 1);
  const expired = { ...v2, occurred_at: new Date(Date.now() - 9 * 86400000).toISOString() };
  assert.equal((await ingest(db, expired)).status, 400);
});
