import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import worker from "../src/worker.js";
import { classifyConcurrency, concurrencyEntries, CONCURRENCY_COUNTS } from "../src/concurrency.js";

const sql = (name) => readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
const schema = sql("schema.sql");
const migration = sql("migrations/0026_concurrency_tracking.sql");
const dashboard = sql("concurrency.sql");
let sequence = 0;
function body(overrides = {}) {
  const n = ++sequence;
  return {
    id: "install-a", event: "session_concurrency", event_id: `event-${n}`,
    session_id: `logical-${n}`, concurrency_session_id: `runtime-${n}`,
    version: "0.0.0-test", os: "linux", arch: "x86_64", is_ci: false,
    build_channel: "ci_release", concurrency_tracking_version: 2,
    concurrency_tracking_scope: "runtime_agent_sessions", concurrency_tracking_available: true,
    phase: "end", agent_role: "root", active_sessions_at_start: 1,
    other_active_sessions_at_start: 0, max_concurrent_sessions: 3, multi_sessioned: true,
    root_sessions_at_start: 1, child_sessions_at_start: 0,
    max_concurrent_root_sessions: 2, max_concurrent_child_sessions: 2,
    ...overrides,
  };
}
function dbWithSchema({ old = false } = {}) {
  const db = new DatabaseSync(":memory:");
  db.exec("PRAGMA foreign_keys = ON");
  db.exec(old ? schema.split("-- Reliable runtime concurrency (migration 0026).")[0] : schema);
  return db;
}
function d1(db, fail = () => false) {
  return { prepare(sql) {
    const execute = (values) => ({
      async run() {
        if (fail(sql)) throw new Error("injected D1 write failure");
        const result = db.prepare(sql).run(...values);
        return { meta: { changes: Number(result.changes), size_after: 1000 } };
      },
      async all() { return { results: db.prepare(sql).all(...values) }; },
    });
    return { ...execute([]), bind: (...values) => execute(values) };
  } };
}
async function ingest(db, value, extra = {}) {
  const request = new Request("https://telemetry.example/v1/event", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(value),
  });
  const response = await worker.fetch(request, { DB: d1(db), ...extra }, {});
  return { status: response.status, ...(await response.json()) };
}
function stored(db, value) {
  return db.prepare("SELECT * FROM concurrency_details WHERE event_id = ?").get(value.event_id);
}
function insertFixture(db, value, { details = true, age = "-1 hour" } = {}) {
  db.prepare(`INSERT INTO events (telemetry_id, event, event_id, session_id,
    version, os, arch, is_ci, build_channel, created_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now', ?))`).run(
    value.id, value.event, value.event_id, value.session_id,
    value.version, value.os, value.arch, Number(!!value.is_ci), value.build_channel, age,
  );
  if (details) {
    const entries = concurrencyEntries(value);
    db.prepare(`INSERT INTO concurrency_details (${entries.map(([k]) => k).join(",")})
      VALUES (${entries.map(() => "?").join(",")})`).run(...entries.map(([, v]) => v));
  }
}
const row = (rows, panel, bucket) => rows.find((r) => r.panel === panel && r.bucket === bucket);

test("strict v2 validation accepts independent root/child peaks and all safe integer counts", () => {
  assert.equal(classifyConcurrency(body()).quality, "trusted");
  assert.equal(classifyConcurrency(body({ agent_role: "child", root_sessions_at_start: 0,
    child_sessions_at_start: 1 })).quality, "trusted");
  assert.equal(classifyConcurrency(body({ phase: "start", max_concurrent_sessions: 1,
    max_concurrent_root_sessions: 1, max_concurrent_child_sessions: 0, multi_sessioned: false })).quality, "trusted");
  for (const peak of [85318, Number.MAX_SAFE_INTEGER]) {
    assert.equal(classifyConcurrency(body({ max_concurrent_sessions: peak,
      max_concurrent_root_sessions: peak, max_concurrent_child_sessions: 0 })).quality, "trusted");
  }
});

test("missing numeric fields are distinct from explicit zero and null", () => {
  for (const field of CONCURRENCY_COUNTS) {
    const value = body();
    delete value[field];
    assert.deepEqual(classifyConcurrency(value), { quality: "missing_fields", reason: field });
    value[field] = null;
    assert.equal(classifyConcurrency(value).quality, "invalid", field);
  }
  assert.equal(classifyConcurrency(body({ active_sessions_at_start: 0 })).quality, "invalid");
  assert.equal(classifyConcurrency(body({ child_sessions_at_start: 0 })).quality, "trusted");
});

test("numeric strings, booleans, fractions, negatives, containers and unsafe integers never become counts", () => {
  for (const field of CONCURRENCY_COUNTS) {
    for (const invalid of ["2", true, false, 1.25, -1, {}, [], NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      assert.equal(classifyConcurrency(body({ [field]: invalid })).quality, "invalid", `${field}=${String(invalid)}`);
    }
  }
});

test("inconsistent, unsupported and ambiguous payloads are explicitly classified", () => {
  const cases = [
    [{ concurrency_tracking_version: 1 }, "legacy_version"],
    [{ concurrency_tracking_version: 3 }, "unsupported_version"],
    [{ concurrency_tracking_version: "2" }, "invalid"],
    [{ concurrency_tracking_version: null }, "invalid"],
    [{ concurrency_tracking_available: false }, "unavailable"],
    [{ concurrency_tracking_available: 1 }, "invalid"],
    [{ event: "session_end" }, "legacy_scope"],
    [{ concurrency_tracking_scope: "legacy_process_global" }, "invalid"],
    [{ max_concurrent_sessions: 0 }, "invalid"],
    [{ other_active_sessions_at_start: 1 }, "invalid"],
    [{ multi_sessioned: false }, "invalid"],
    [{ multi_sessioned: 1 }, "invalid"],
    [{ phase: "start" }, "invalid"],
    [{ phase: "crash" }, "invalid"],
    [{ agent_role: "worker" }, "invalid"],
    [{ agent_role: "child" }, "invalid"],
    [{ child_sessions_at_start: 1 }, "invalid"],
    [{ max_concurrent_root_sessions: 4 }, "invalid"],
    [{ max_concurrent_child_sessions: 0, max_concurrent_root_sessions: 1 }, "invalid"],
    [{ concurrency_session_id: " " }, "invalid"],
    [{ session_id: "" }, "invalid"],
    [{ is_ci: "false" }, "invalid"],
  ];
  for (const [override, expected] of cases) assert.equal(classifyConcurrency(body(override)).quality, expected, JSON.stringify(override));
  const value = body(); delete value.concurrency_tracking_version;
  assert.equal(classifyConcurrency(value).quality, "missing_version");
  delete value.is_ci; value.concurrency_tracking_version = 2;
  assert.equal(classifyConcurrency(value).quality, "missing_fields");
});

test("real worker intake persists v2 and quarantines bad counts without losing valid events", async () => {
  const db = dbWithSchema();
  for (const invalid of ["3", 1.5, -1, null, {}, [], 0, Number.MAX_SAFE_INTEGER + 1]) {
    const value = body({ max_concurrent_sessions: invalid });
    const response = await ingest(db, value);
    assert.equal(response.status, 200);
    assert.equal(response.durable, true);
    assert.equal(response.concurrency_durable, true);
    const detail = stored(db, value);
    assert.equal(detail.quality, "invalid");
    assert.deepEqual(JSON.parse(detail.raw_json).max_concurrent_sessions, invalid);
    assert.equal(db.prepare("SELECT count(*) AS n FROM events WHERE event_id = ?").get(value.event_id).n, 1);
  }
  const value = body();
  assert.equal((await ingest(db, value)).concurrency_durable, true);
  assert.equal(stored(db, value).concurrency_tracking_version, 2);
  assert.equal(stored(db, value).quality, "trusted");
  await ingest(db, value);
  assert.equal(db.prepare("SELECT count(*) AS n FROM concurrency_details WHERE event_id = ?").get(value.event_id).n, 1);
  assert.equal(db.prepare("SELECT count(*) AS n FROM trusted_concurrency_events").get().n, 1);
  db.close();
});

test("legacy lifecycle fields survive and missing values do not become zero", async () => {
  const db = dbWithSchema();
  const value = body({ event: "session_end", max_concurrent_sessions: 85318 });
  delete value.concurrency_tracking_version;
  await ingest(db, value);
  const legacy = db.prepare("SELECT * FROM session_details WHERE event_id = ?").get(value.event_id);
  assert.equal(legacy.max_concurrent_sessions, 85318);
  assert.equal(stored(db, value).quality, "missing_version");
  const absent = body({ event: "session_end", concurrency_tracking_scope: "legacy_process_global", concurrency_tracking_available: false });
  for (const key of [...CONCURRENCY_COUNTS, "multi_sessioned"]) delete absent[key];
  await ingest(db, absent);
  assert.equal(stored(db, absent).quality, "legacy_scope");
  assert.equal(db.prepare("SELECT max_concurrent_sessions FROM session_details WHERE event_id = ?").get(absent.event_id).max_concurrent_sessions, null);
  assert.equal(Object.hasOwn(JSON.parse(stored(db, absent).raw_json), "max_concurrent_sessions"), false);
  const malformed = body({ event: "session_end", max_concurrent_sessions: { invalid: true } });
  assert.equal((await ingest(db, malformed)).durable, true);
  assert.equal(db.prepare("SELECT max_concurrent_sessions FROM session_details WHERE event_id = ?").get(malformed.event_id).max_concurrent_sessions, '{"invalid":true}');
  assert.equal(db.prepare("SELECT count(*) AS n FROM trusted_concurrency_events").get().n, 0);
  db.close();
});

test("migration is additive, repeatable, under column limits, and cascades retained detail deletion", () => {
  const db = dbWithSchema({ old: true });
  const legacy = body({ event: "session_end" });
  insertFixture(db, legacy, { details: false });
  db.prepare("INSERT INTO session_details (event_id, max_concurrent_sessions) VALUES (?, 85318)").run(legacy.event_id);
  const before = db.prepare("SELECT * FROM events").all();
  db.exec(migration); db.exec(migration);
  assert.deepEqual(db.prepare("SELECT * FROM events").all(), before);
  assert.equal(db.prepare("SELECT max_concurrent_sessions FROM session_details").get().max_concurrent_sessions, 85318);
  assert.equal(db.prepare("SELECT quality FROM concurrency_event_quality").get().quality, "legacy_unclassified");
  assert.equal(db.prepare("SELECT count(*) AS n FROM trusted_concurrency_events").get().n, 0);
  assert.ok(db.prepare("PRAGMA table_info(concurrency_details)").all().length < 100);
  const value = body(); insertFixture(db, value);
  db.prepare("DELETE FROM events WHERE event_id = ?").run(value.event_id);
  assert.equal(stored(db, value), undefined);
  assert.deepEqual(db.prepare("PRAGMA foreign_key_check").all(), []);
  const fresh = dbWithSchema();
  assert.deepEqual(db.prepare("SELECT name, sql FROM sqlite_master WHERE name LIKE '%concurrency%' ORDER BY name").all(),
    fresh.prepare("SELECT name, sql FROM sqlite_master WHERE name LIKE '%concurrency%' ORDER BY name").all());
  fresh.close(); db.close();
});

test("missing migration or failed details never claim concurrency durability, retry repairs detail", async () => {
  const db = dbWithSchema({ old: true });
  const value = body();
  const response = await ingest(db, value);
  assert.equal(response.status, 503); assert.equal(response.durable, true);
  assert.equal(response.concurrency_durable, false); assert.equal(response.firehose, false);
  db.exec(migration);
  assert.equal(db.prepare("SELECT quality FROM concurrency_event_quality").get().quality, "missing_detail");
  assert.equal((await ingest(db, value)).concurrency_durable, true);
  assert.equal(stored(db, value).quality, "trusted");
  const legacyDb = dbWithSchema({ old: true });
  const legacy = await ingest(legacyDb, body({ event: "session_end" }));
  assert.equal(legacy.status, 200); assert.equal(legacy.durable, true);
  assert.equal(legacy.concurrency_durable, false);
  legacyDb.close();
  db.close();
});

test("dedicated firehose preserves raw metrics and never claims generic firehose is sufficient", async () => {
  const db = dbWithSchema(); const points = [];
  const failing = d1(db, (query) => /^INSERT/i.test(query.trim()));
  const value = body({ max_concurrent_sessions: "not-a-count" });
  const response = await ingest(db, value, { DB: failing,
    FIREHOSE_CONCURRENCY: { writeDataPoint(point) { points.push(point); } } });
  assert.equal(response.status, 200); assert.equal(response.durable, false);
  assert.equal(response.concurrency_durable, false); assert.equal(response.firehose, true);
  assert.equal(JSON.parse(points[0].blobs[9]).max_concurrent_sessions, "not-a-count");
  assert.equal(points[0].blobs.length, 16);
  assert.equal(points[0].blobs[10], "invalid");
  assert.equal(points[0].doubles.length, 11);
  assert.equal(points[0].doubles[5], 0, "unrepresentable copy has a non-trusted quality guard");
  const missing = await ingest(db, body(), { DB: failing, FIREHOSE: { writeDataPoint() { throw Error("must not be called"); } } });
  assert.equal(missing.status, 503);
  const oversized = await ingest(db, body({ max_concurrent_sessions: "x".repeat(17000) }), {
    FIREHOSE_CONCURRENCY: { writeDataPoint() { throw Error("oversized point must not be written"); } },
  });
  assert.equal(oversized.status, 200); assert.equal(oversized.firehose, false);
  assert.equal(oversized.concurrency_durable, true);
  db.close();
});

test("detail failure after a successful parent is retryable unless the full firehose point lands", async () => {
  const db = dbWithSchema();
  const value = body();
  const extra = { DB: d1(db, (query) => /INSERT.*concurrency_details/.test(query)) };
  const failed = await ingest(db, value, extra);
  assert.equal(failed.status, 503); assert.equal(failed.durable, true);
  assert.equal(failed.concurrency_durable, false); assert.equal(failed.firehose, false);
  assert.equal(db.prepare("SELECT count(*) AS n FROM events WHERE event_id = ?").get(value.event_id).n, 1);
  const points = [];
  const fallback = await ingest(db, value, { ...extra,
    FIREHOSE_CONCURRENCY: { writeDataPoint(point) { points.push(point); } } });
  assert.equal(fallback.status, 200); assert.equal(fallback.concurrency_durable, false);
  assert.equal(fallback.firehose, true); assert.equal(points.length, 1);
  assert.equal(points[0].blobs[10], "trusted");
  assert.equal(points[0].blobs[12], "runtime_agent_sessions");
  assert.equal(points[0].blobs[13], "end");
  assert.equal(points[0].blobs[14], "root");
  assert.equal(points[0].blobs[15], value.concurrency_session_id);
  assert.deepEqual(points[0].doubles, [2, 1, 0, 1, 0, 3, 1, 0, 2, 2, 1]);
  assert.equal((await ingest(db, value)).concurrency_durable, true);
  assert.equal(stored(db, value).quality, "trusted");
  assert.equal(db.prepare("SELECT count(*) AS n FROM events WHERE event_id = ?").get(value.event_id).n, 1);
  db.close();
});

test("real dashboard excludes legacy, invalid, missing, future versions and CI with explicit coverage", () => {
  const db = dbWithSchema();
  insertFixture(db, body({ concurrency_session_id: "paired", phase: "start", max_concurrent_sessions: 1,
    max_concurrent_root_sessions: 1, max_concurrent_child_sessions: 0, multi_sessioned: false }));
  insertFixture(db, body({ concurrency_session_id: "paired" }));
  insertFixture(db, body({ concurrency_session_id: "paired" })); // duplicate phase, distinct event_id
  insertFixture(db, body({ max_concurrent_sessions: 7, max_concurrent_root_sessions: 6 }));
  insertFixture(db, body({ id: "install-b", max_concurrent_sessions: 1, multi_sessioned: false,
    max_concurrent_root_sessions: 1, max_concurrent_child_sessions: 0 }));
  insertFixture(db, body({ phase: "start", max_concurrent_sessions: 1, multi_sessioned: false,
    max_concurrent_root_sessions: 1, max_concurrent_child_sessions: 0 }));
  insertFixture(db, body({ is_ci: true, max_concurrent_sessions: 10000, max_concurrent_root_sessions: 10000 }));
  insertFixture(db, body({ concurrency_tracking_version: 1 }));
  insertFixture(db, body({ concurrency_tracking_version: 3 }));
  insertFixture(db, body({ max_concurrent_sessions: 0 }));
  const missing = body(); delete missing.max_concurrent_sessions; insertFixture(db, missing);
  insertFixture(db, body({ concurrency_tracking_available: false }));
  insertFixture(db, body(), { details: false });
  insertFixture(db, body({ event: "session_end", max_concurrent_sessions: 85318 }), { details: false });
  insertFixture(db, body({ max_concurrent_sessions: 1000, max_concurrent_root_sessions: 1000 }), { age: "-31 days" });
  const rows = db.prepare(dashboard).all();
  const sessions = row(rows, "peak_summary", "runtime_sessions_with_trusted_end");
  assert.equal(sessions.observations, 3); assert.equal(sessions.avg_peak, 3.67); assert.equal(sessions.max_peak, 7);
  const installs = row(rows, "peak_summary", "installations_with_trusted_end");
  assert.equal(installs.observations, 2); assert.equal(installs.avg_peak, 4); assert.equal(installs.max_peak, 7);
  assert.equal(row(rows, "completion_coverage", "start_without_end").observations, 1);
  assert.equal(row(rows, "completion_coverage", "end_without_start").observations, 2);
  const coverage = row(rows, "coverage_summary", "dedicated_non_ci_events");
  assert.equal(coverage.observations, 12); assert.equal(coverage.trusted_pct, 50);
  assert.equal(row(rows, "coverage_by_source", "session_end:legacy_unclassified:non_ci").observations, 1);
  assert.equal(row(rows, "coverage_by_source", "session_concurrency:trusted:ci").observations, 1);
  assert.equal(rows.filter((r) => r.panel === "session_end_peak").reduce((n, r) => n + r.observations, 0), 3);
  assert.equal(rows.filter((r) => r.panel === "installation_observed_peak").reduce((n, r) => n + r.observations, 0), 2);
  db.close();
});

test("trusted view independently rejects corrupted typed columns even with a trusted label", () => {
  const db = dbWithSchema();
  for (const [field, value] of [["max_concurrent_sessions", 0], ["active_sessions_at_start", null],
    ["max_concurrent_sessions", 2.5], ["multi_sessioned", 0], ["runtime_is_ci", null],
    ["concurrency_tracking_version", 1], ["child_sessions_at_start", -1]]) {
    const event = body(); insertFixture(db, event);
    db.prepare(`UPDATE concurrency_details SET ${field} = ? WHERE event_id = ?`).run(value, event.event_id);
  }
  assert.equal(db.prepare("SELECT count(*) AS n FROM trusted_concurrency_events").get().n, 0);
  assert.deepEqual([...new Set(db.prepare("SELECT quality FROM concurrency_event_quality").all().map((r) => r.quality))], ["invalid_storage"]);
  db.close();
});

test("empty clean dataset reports unknown peak and coverage, not zero concurrency", () => {
  const db = dbWithSchema();
  const rows = db.prepare(dashboard).all();
  assert.equal(row(rows, "coverage_summary", "dedicated_non_ci_events").trusted_pct, null);
  assert.equal(row(rows, "peak_summary", "runtime_sessions_with_trusted_end").observations, 0);
  assert.equal(row(rows, "peak_summary", "runtime_sessions_with_trusted_end").max_peak, null);
  db.close();
});

test("nightly retention removes expired concurrency details and preserves fresh observations", async () => {
  const db = dbWithSchema();
  const old = body(); insertFixture(db, old, { age: "-366 days" });
  const fresh = body(); insertFixture(db, fresh);
  const waited = [];
  await worker.scheduled({}, { DB: d1(db) }, { waitUntil(promise) { waited.push(promise); } });
  await Promise.all(waited);
  assert.equal(stored(db, old), undefined);
  assert.equal(stored(db, fresh).quality, "trusted");
  assert.deepEqual(db.prepare("PRAGMA foreign_key_check").all(), []);
  db.close();
});

test("dashboard stays within a five-term SQLite compound SELECT budget", (t) => {
  // Node's sqlite binding does not expose sqlite3_limit. Python 3.11+ does.
  // D1's lower limit broke the original 11-way UNION despite desktop tests.
  const check = spawnSync("python3", ["-c", `
import json, sqlite3, sys
if not hasattr(sqlite3.Connection, 'setlimit'):
    sys.exit(77)
payload = json.load(sys.stdin)
db = sqlite3.connect(':memory:')
db.executescript(payload['schema'])
db.setlimit(sqlite3.SQLITE_LIMIT_COMPOUND_SELECT, 5)
try:
    db.execute(' UNION ALL '.join(['SELECT 1'] * 6))
    raise AssertionError('compound limit was not enforced')
except sqlite3.OperationalError as error:
    assert 'too many terms' in str(error)
rows = db.execute(payload['dashboard']).fetchall()
assert len(rows) >= 8, rows
print('compound_limit=5, dashboard_rows=' + str(len(rows)))
`], { input: JSON.stringify({ schema, dashboard }), encoding: "utf8" });
  if (check.error?.code === "ENOENT" || check.status === 77) {
    t.skip("Python 3.11+ required to lower SQLite compound-select limit");
    return;
  }
  assert.equal(check.status, 0, check.stderr || String(check.error));
  assert.match(check.stdout, /compound_limit=5/);
});
