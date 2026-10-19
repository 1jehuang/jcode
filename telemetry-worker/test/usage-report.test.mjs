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
  await worker.scheduled({}, { DB: d1(db) }, { waitUntil(promise) { jobs.push(promise); } });
  await Promise.all(jobs);
  assert.equal(db.prepare("SELECT count(*) n FROM usage_report_ledger").get().n, 0);
  assert.equal(db.prepare("SELECT responses FROM daily_model_usage_v2").get().responses, 1);
  const expired = { ...v2, occurred_at: new Date(Date.now() - 9 * 86400000).toISOString() };
  assert.equal((await ingest(db, expired)).status, 400);
});
