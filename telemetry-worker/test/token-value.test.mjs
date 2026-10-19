// Execute all three checked-in dashboards against real in-memory SQLite.
// Prices and model names below are synthetic fixtures, not current list rates.
// Legacy session counters can contain overlapping cache reads. Contract-v2
// response rollups are already disjoint and must never be corrected twice.
// Run with: node --test test/token-value.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");

const DASHBOARD_SQL = readFileSync(join(root, "token-value.sql"), "utf8");
const DAILY_SQL = readFileSync(join(root, "token-value-daily.sql"), "utf8");
const MODEL_SQL = readFileSync(join(root, "model-usage.sql"), "utf8");
const USAGE_MIGRATION = readFileSync(
  join(root, "migrations", "0027_daily_model_usage.sql"), "utf8",
);
const RECEIPTS_MIGRATION = readFileSync(
  join(root, "migrations", "0031_usage_report_receipts.sql"), "utf8",
);
const PRICES_MIGRATION = readFileSync(
  join(root, "migrations", "0023_model_prices.sql"),
  "utf8",
);

// Minimal `events` shape: only the columns token-value.sql touches.
const EVENTS_DDL = `
CREATE TABLE events (
    event TEXT,
    created_at TEXT,
    model_end TEXT,
    provider_end TEXT,
    telemetry_id TEXT,
    is_ci INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_input_tokens INTEGER DEFAULT 0,
    cache_creation_input_tokens INTEGER DEFAULT 0
);`;

function makeDb() {
  const db = new DatabaseSync(":memory:");
  db.exec(EVENTS_DDL);
  db.exec(PRICES_MIGRATION);
  db.exec(USAGE_MIGRATION);
  db.exec(RECEIPTS_MIGRATION);
  return db;
}

function insertPrice(db, row) {
  db.prepare(
    `INSERT INTO model_prices (model, input_usd_per_mtok, output_usd_per_mtok,
       cache_read_usd_per_mtok, cache_write_usd_per_mtok,
       input_includes_cache_read, price_kind)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    row.model,
    row.input ?? null,
    row.output ?? null,
    row.cacheRead ?? null,
    row.cacheWrite ?? null,
    row.inputIncludesCacheRead ?? 0,
    row.priceKind ?? "catalog",
  );
}

function insertSession(db, row) {
  db.prepare(
    `INSERT INTO events (event, created_at, model_end, provider_end, is_ci,
       input_tokens, output_tokens, cache_read_input_tokens,
       cache_creation_input_tokens, telemetry_id)
     VALUES ('session_end', datetime('now'), ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    row.model,
    row.provider ?? "TestProvider",
    row.isCi ?? 0,
    row.input ?? 0,
    row.output ?? 0,
    row.cacheRead ?? 0,
    row.cacheWrite ?? 0,
    row.user ?? "user-1",
  );
}

function runDashboard(db) {
  return db.prepare(DASHBOARD_SQL).all();
}

function modelRows(rows) {
  return rows.filter((r) => r.panel === "model_7d");
}

function summary(rows, bucket) {
  return rows.find((r) => r.panel === "summary" && r.bucket === bucket);
}

test("cache reads are not double charged for OpenAI-style providers", () => {
  const db = makeDb();
  // Synthetic OpenAI-style rates. input_tokens includes the cached portion.
  insertPrice(db, {
    model: "gpt-5.6-sol",
    input: 5,
    output: 30,
    cacheRead: 0.5,
    cacheWrite: 6.25,
    inputIncludesCacheRead: 1,
  });
  // 1M prompt tokens of which 900k were cache hits, plus 10k output.
  insertSession(db, {
    model: "gpt-5.6-sol",
    input: 1_000_000,
    cacheRead: 900_000,
    output: 10_000,
  });

  const row = modelRows(runDashboard(db))[0];
  // 100k fresh input @ $5 = $0.50, 900k cache read @ $0.50 = $0.45,
  // 10k output @ $30 = $0.30. Total $1.25.
  assert.equal(row.usd_value, 1.25);
  assert.equal(row.input_tokens, 100_000, "cached portion must leave the input bucket");
});

test("Anthropic-style disjoint buckets keep their full input count", () => {
  const db = makeDb();
  // Synthetic Anthropic-style rates with cache reads reported separately.
  insertPrice(db, {
    model: "claude-opus-5",
    input: 5,
    output: 25,
    cacheRead: 0.5,
    cacheWrite: 6.25,
    inputIncludesCacheRead: 0,
  });
  insertSession(db, {
    model: "claude-opus-5",
    input: 100_000,
    cacheRead: 900_000,
    output: 10_000,
  });

  const row = modelRows(runDashboard(db))[0];
  // 100k input @ $5 = $0.50, 900k cache read @ $0.50 = $0.45,
  // 10k output @ $25 = $0.25. Total $1.20.
  assert.equal(row.usd_value, 1.2);
  assert.equal(row.input_tokens, 100_000, "disjoint input must not be reduced");
});

test("the two provider conventions agree on identical real usage", () => {
  // The same underlying work reported under each convention must price the
  // same. This is the invariant that the 10x overcharge bug would break.
  const db = makeDb();
  const rates = { input: 5, output: 30, cacheRead: 0.5, cacheWrite: 6.25 };
  insertPrice(db, { model: "openai-style", ...rates, inputIncludesCacheRead: 1 });
  insertPrice(db, { model: "anthropic-style", ...rates, inputIncludesCacheRead: 0 });
  insertSession(db, {
    model: "openai-style",
    input: 1_000_000,
    cacheRead: 900_000,
    output: 10_000,
  });
  insertSession(db, {
    model: "anthropic-style",
    input: 100_000,
    cacheRead: 900_000,
    output: 10_000,
  });

  const rows = modelRows(runDashboard(db));
  const openai = rows.find((r) => r.bucket.startsWith("openai-style"));
  const anthropic = rows.find((r) => r.bucket.startsWith("anthropic-style"));
  assert.equal(openai.usd_value, anthropic.usd_value);
});

test("unpriced models contribute no dollars but are counted as unpriced tokens", () => {
  const db = makeDb();
  insertPrice(db, { model: "someones-private-alias", priceKind: "unpriced" });
  insertSession(db, {
    model: "someones-private-alias",
    input: 1_000_000,
    output: 10_000,
  });

  const rows = runDashboard(db);
  const row = modelRows(rows)[0];
  assert.equal(row.usd_value, 0, "no price means no dollars, never a guess");
  assert.equal(row.unpriced_tokens, 1_010_000);
  assert.equal(summary(rows, "last_24h").priced_token_pct, 0);
});

test("models with no price row at all are treated as unpriced, not free", () => {
  // A model label observed before the next sync run has no model_prices row.
  // The LEFT JOIN must surface it as unpriced rather than quietly zero-rating
  // it, otherwise coverage looks perfect while dollars go missing.
  const db = makeDb();
  insertSession(db, { model: "brand-new-model", input: 500_000, output: 5_000 });

  const rows = runDashboard(db);
  assert.equal(modelRows(rows)[0].unpriced_tokens, 505_000);
  assert.equal(summary(rows, "last_24h").priced_token_pct, 0);
});

test("free routes are priced at zero without polluting the unpriced count", () => {
  const db = makeDb();
  insertPrice(db, {
    model: "some-model:free",
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    priceKind: "free",
  });
  insertSession(db, { model: "some-model:free", input: 1_000_000, output: 10_000 });

  const rows = runDashboard(db);
  const row = modelRows(rows)[0];
  assert.equal(row.usd_value, 0);
  assert.equal(row.unpriced_tokens, 0, "genuinely free is not the same as unknown");
});

test("CI traffic is excluded from the dollar figure", () => {
  // Ephemeral CI runners are not user demand, and health.sql already treats
  // them as noise; the value dashboard must agree.
  const db = makeDb();
  insertPrice(db, { model: "gpt-5.6-sol", input: 5, output: 30, cacheRead: 0.5 });
  insertSession(db, { model: "gpt-5.6-sol", input: 1_000_000, isCi: 1 });

  assert.equal(modelRows(runDashboard(db)).length, 0);
});

test("cache writes are billed at the write rate, not the read rate", () => {
  const db = makeDb();
  insertPrice(db, {
    model: "claude-opus-5",
    input: 5,
    output: 25,
    cacheRead: 0.5,
    cacheWrite: 6.25,
  });
  insertSession(db, { model: "claude-opus-5", cacheWrite: 1_000_000 });

  assert.equal(modelRows(runDashboard(db))[0].usd_value, 6.25);
});

test("the 24h summary uses a rolling window, not two partial calendar days", () => {
  // `date('now','-1 days')` spans yesterday-plus-today and roughly doubles the
  // reported figure; the panel must use a true 24-hour window.
  const db = makeDb();
  insertPrice(db, { model: "m", input: 10, output: 10, cacheRead: 1 });
  db.prepare(
    `INSERT INTO events (event, created_at, model_end, provider_end, input_tokens)
     VALUES ('session_end', datetime('now', '-40 hours'), 'm', 'p', 1000000)`,
  ).run();
  insertSession(db, { model: "m", input: 1_000_000 });

  const rows = runDashboard(db);
  assert.equal(summary(rows, "last_24h").usd_value, 10, "the 40h-old row must be excluded");
  assert.equal(summary(rows, "last_30d_total").usd_value, 20, "but still counted in 30d");
});

test("the run rate is the 7-day mean and the projection is 30x it", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 7, output: 0, cacheRead: 0 });
  insertSession(db, { model: "m", input: 1_000_000 });

  const rows = runDashboard(db);
  const runRate = summary(rows, "run_rate_usd_per_day_7d").usd_value;
  const projection = summary(rows, "projected_usd_per_month_from_7d").usd_value;
  assert.equal(runRate, 1, "$7 of usage over a 7-day window is $1/day");
  assert.equal(projection, 30);
});

test("the 7-day window is a rolling 168 hours, not 8 calendar days", () => {
  // Regression: a `day >= date('now','-7 days')` filter includes both the
  // boundary day and today, so it spans 8 partial calendar days. Dividing that
  // by 7 overstates the run rate, and the monthly projection inherits it.
  const db = makeDb();
  insertPrice(db, { model: "m", input: 7, output: 0, cacheRead: 0 });
  // One session per day for the last 10 days, $7 of usage each.
  for (let daysAgo = 0; daysAgo < 10; daysAgo += 1) {
    db.prepare(
      `INSERT INTO events (event, created_at, model_end, provider_end, input_tokens)
       VALUES ('session_end', datetime('now', '-' || ? || ' hours'), 'm', 'p', 1000000)`,
    ).run(daysAgo * 24 + 1);
  }

  const rows = runDashboard(db);
  // Exactly 7 sessions fall inside the trailing 168 hours: $49 total and a
  // $7/day run rate. The 8-calendar-day form would report $8/day.
  assert.equal(summary(rows, "run_rate_usd_per_day_7d").usd_value, 7);
  assert.equal(summary(rows, "projected_usd_per_month_from_7d").usd_value, 210);
  // Panel 2 must share the window, else per-model dollars will not reconcile
  // against the run rate.
  const modelTotal = modelRows(rows).reduce((sum, r) => sum + r.usd_value, 0);
  assert.equal(modelTotal, 49);
});

// --- token-value-daily.sql: the plain per-day time series -------------------

function runDaily(db) {
  return db.prepare(DAILY_SQL).all();
}

test("the daily series returns one row per day in date order", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 10, output: 0, cacheRead: 0 });
  // Two sessions today, one yesterday, inserted newest-first so a missing
  // ORDER BY or a dollar-sorted result would show up.
  insertSession(db, { model: "m", input: 1_000_000 });
  insertSession(db, { model: "m", input: 1_000_000 });
  db.prepare(
    `INSERT INTO events (event, created_at, model_end, provider_end, input_tokens, telemetry_id)
     VALUES ('session_end', datetime('now', '-1 days'), 'm', 'p', 1000000, 'user-2')`,
  ).run();

  const rows = runDaily(db);
  assert.equal(rows.length, 2);
  assert.ok(rows[0].day < rows[1].day, "rows must be in ascending date order");
  assert.equal(rows[1].usd, 20, "today's two sessions are $10 each");
  assert.equal(rows[1].sessions, 2);
});

test("the daily series counts distinct users", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 10, output: 0, cacheRead: 0 });
  // Three sessions from two distinct users.
  insertSession(db, { model: "m", input: 1_000_000, user: "a" });
  insertSession(db, { model: "m", input: 1_000_000, user: "a" });
  insertSession(db, { model: "m", input: 1_000_000, user: "b" });

  const row = runDaily(db)[0];
  assert.equal(row.usd, 30);
  assert.equal(row.sessions, 3);
  assert.equal(row.users, 2);
  // Deliberately no per-user dollar column: it duplicated tokens-per-user.
  assert.equal(row.usd_per_user, undefined);
});

test("the daily series agrees with the token-value panel for the same day", () => {
  // The two dashboards must never disagree about a day's dollar value, which
  // is the risk of maintaining the pricing expression in two files.
  const db = makeDb();
  insertPrice(db, {
    model: "openai-style",
    input: 5,
    output: 30,
    cacheRead: 0.5,
    cacheWrite: 6.25,
    inputIncludesCacheRead: 1,
  });
  insertPrice(db, {
    model: "anthropic-style",
    input: 5,
    output: 25,
    cacheRead: 0.5,
    cacheWrite: 6.25,
    inputIncludesCacheRead: 0,
  });
  insertSession(db, {
    model: "openai-style",
    input: 1_000_000,
    cacheRead: 900_000,
    output: 10_000,
    cacheWrite: 50_000,
  });
  insertSession(db, {
    model: "anthropic-style",
    input: 100_000,
    cacheRead: 900_000,
    output: 10_000,
  });

  const dailyTotal = runDaily(db).reduce((sum, r) => sum + r.usd, 0);
  const panelTotal = summary(runDashboard(db), "last_24h").usd_value;
  assert.equal(dailyTotal.toFixed(2), panelTotal.toFixed(2));
});

test("the daily series excludes CI and reports its price coverage", () => {
  const db = makeDb();
  insertPrice(db, { model: "priced", input: 10, output: 0, cacheRead: 0 });
  insertSession(db, { model: "priced", input: 1_000_000 });
  insertSession(db, { model: "no-price-row", input: 1_000_000 });
  insertSession(db, { model: "priced", input: 5_000_000, isCi: 1 });

  const row = runDaily(db)[0];
  assert.equal(row.usd, 10, "CI sessions must not add dollars");
  assert.equal(row.sessions, 2, "CI sessions must not be counted");
  assert.equal(row.priced_pct, 50, "half the tokens came from an unpriced model");
});

function insertUsage(db, row) {
  db.prepare(`INSERT INTO daily_model_usage_v2
    (usage_date, source, provider, model, build_channel, is_ci, responses,
     input_tokens, output_tokens, cache_read_input_tokens,
     cache_creation_input_tokens, total_tokens)
    VALUES (date('now'), ?, 'test', ?, ?, ?, 1, ?, ?, ?, ?, ?)`)
    .run(row.source ?? "agent", row.model, row.channel ?? "stable", row.isCi ?? 0,
      row.input ?? 0, row.output ?? 0, row.cacheRead ?? 0, row.cacheWrite ?? 0,
      (row.input ?? 0) + (row.output ?? 0) + (row.cacheRead ?? 0) + (row.cacheWrite ?? 0));
}

test("authoritative SQL never subtracts cache from already disjoint response input", () => {
  const db = makeDb();
  const rates = { input: 5, output: 30, cacheRead: 0.5, cacheWrite: 6.25 };
  insertPrice(db, { model: "subset-legacy", ...rates, inputIncludesCacheRead: 1 });
  insertPrice(db, { model: "disjoint-legacy", ...rates });
  for (const model of ["subset-legacy", "disjoint-legacy"]) {
    insertUsage(db, { model, input: 100_000, output: 10_000, cacheRead: 900_000 });
  }
  // Neither old mixed-semantics rollups nor legacy/raw events feed this query.
  db.exec(`INSERT INTO daily_model_usage
    (usage_date, source, provider, model, input_tokens)
    VALUES (date('now'), 'agent', 'test', 'subset-legacy', 99000000)`);
  insertSession(db, { model: "subset-legacy", input: 99_000_000 });
  db.exec(`INSERT INTO events (event, created_at, input_tokens)
    VALUES ('usage_report', datetime('now'), 99000000)`);
  const [row] = db.prepare(MODEL_SQL).all();
  assert.equal(row.responses, 2);
  assert.equal(row.tokens, 2_020_000);
  assert.equal(row.list_usd, 2.5);
  assert.equal(row.unpriced_tokens, 0);
});

test("response SQL separates sources, combines build channels and excludes CI", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 5 });
  for (const source of ["agent", "compaction", "sidecar"]) {
    insertUsage(db, { model: "m", source, input: 1_000_000 });
  }
  insertUsage(db, { model: "m", channel: "canary", input: 1_000_000 });
  insertUsage(db, { model: "m", isCi: 1, input: 9_000_000 });
  const rows = db.prepare(MODEL_SQL).all();
  assert.equal(rows.length, 3);
  assert.equal(rows.find(r => r.source === "agent").list_usd, 10);
  assert.equal(rows.find(r => r.source === "agent").responses, 2);
  assert.equal(rows.find(r => r.source === "compaction").list_usd, 5);
  assert.equal(rows.find(r => r.source === "sidecar").list_usd, 5);
});

test("legacy daily totals reconcile with panels after cache-subset correction", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 5, output: 30, cacheRead: 0.5,
    cacheWrite: 6.25, inputIncludesCacheRead: 1 });
  insertSession(db, { model: "m", input: 1_000_000, output: 10_000,
    cacheRead: 900_000, cacheWrite: 50_000 });
  assert.equal(runDaily(db)[0].tokens, 1_060_000);
  assert.equal(summary(runDashboard(db), "last_24h").tokens, 1_060_000);
  assert.equal(runDaily(db)[0].usd, 1.56);
});

for (const [bucket, counts] of [
  ["input", { input: 1_000_000 }],
  ["output", { output: 1_000_000 }],
  ["cacheRead", { cacheRead: 1_000_000 }],
  ["cacheWrite", { cacheWrite: 1_000_000 }],
]) {
  test(`missing used ${bucket} rate is unknown in all three real SQL files`, () => {
    const db = makeDb();
    insertPrice(db, { model: "partial", input: 5, output: 30,
      cacheRead: 0.5, cacheWrite: 6.25, [bucket]: null });
    insertSession(db, { model: "partial", ...counts });
    insertUsage(db, { model: "partial", ...counts });
    const panel = summary(runDashboard(db), "last_24h");
    const daily = runDaily(db)[0];
    const usage = db.prepare(MODEL_SQL).get();
    assert.equal(panel.usd_value, 0);
    assert.equal(daily.usd, 0);
    assert.equal(usage.list_usd, 0);
    for (const row of [panel, daily, usage]) assert.equal(row.unpriced_tokens, 1_000_000);
    assert.equal(panel.priced_token_pct, 0);
    assert.equal(daily.priced_pct, 0);
  });
}

test("unused missing rates do not invalidate known prices, and explicit free is covered", () => {
  const db = makeDb();
  insertPrice(db, { model: "input-only", input: 5 });
  insertPrice(db, { model: "free", input: 0, output: 0, cacheRead: 0,
    cacheWrite: 0, priceKind: "free" });
  for (const model of ["input-only", "free"]) {
    insertSession(db, { model, input: 1_000_000 });
    insertUsage(db, { model, input: 1_000_000 });
  }
  assert.equal(runDaily(db)[0].priced_pct, 100);
  assert.equal(summary(runDashboard(db), "last_24h").priced_token_pct, 100);
  assert.equal(db.prepare(MODEL_SQL).get().list_usd, 5);
  assert.equal(db.prepare(MODEL_SQL).get().unpriced_tokens, 0);
});

test("catalog labels without rates and unpriced labels with stale rates remain unknown", () => {
  const db = makeDb();
  insertPrice(db, { model: "empty-catalog" });
  insertPrice(db, { model: "stale", input: 5, priceKind: "unpriced" });
  for (const model of ["empty-catalog", "stale", "no-row"]) {
    insertSession(db, { model, input: 1_000_000 });
    insertUsage(db, { model, input: 1_000_000 });
  }
  assert.equal(runDaily(db)[0].unpriced_tokens, 3_000_000);
  assert.equal(summary(runDashboard(db), "last_24h").usd_value, 0);
  assert.equal(db.prepare(MODEL_SQL).get().unpriced_tokens, 3_000_000);
});

test("fully cached legacy usage does not require an unused fresh input rate", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", cacheRead: 0.5, inputIncludesCacheRead: 1 });
  insertSession(db, { model: "m", input: 1_000_000, cacheRead: 1_000_000 });
  assert.equal(runDaily(db)[0].usd, 0.5);
  assert.equal(summary(runDashboard(db), "last_24h").usd_value, 0.5);
});

test("legacy cache subtraction clamps to zero, includes writes and ignores turn duplicates", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 5, cacheRead: 0.5,
    cacheWrite: 6.25, inputIncludesCacheRead: 1 });
  insertSession(db, { model: "m", input: 1, cacheRead: 1_000_000, cacheWrite: 1_000_000 });
  db.exec(`INSERT INTO events (event, created_at, model_end, input_tokens)
    VALUES ('turn_end', datetime('now'), 'm', 99000000)`);
  assert.equal(runDaily(db)[0].tokens, 2_000_000);
  assert.equal(summary(runDashboard(db), "last_24h").tokens, 2_000_000);
  assert.equal(runDaily(db)[0].usd, 6.75);
});

test("token run rates preserve fractions instead of truncating before projection", () => {
  const db = makeDb();
  insertSession(db, { model: "unknown", input: 1 });
  const rows = runDashboard(db);
  assert.equal(summary(rows, "run_rate_usd_per_day_7d").tokens, 1 / 7);
  assert.equal(summary(rows, "run_rate_usd_per_day_7d").unpriced_tokens, 1 / 7);
  assert.equal(summary(rows, "projected_usd_per_month_from_7d").tokens, 1 / 7 * 30);
});

test("ISO timestamps respect rolling 24h, 7d, 30d and 60d cutoffs", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 7 });
  for (const hours of [1, 25, 169, 721, 1441]) {
    db.prepare(`INSERT INTO events (event, created_at, model_end, input_tokens)
      VALUES ('session_end', strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-' || ? || ' hours'),
        'm', 1000000)`).run(hours);
  }
  const rows = runDashboard(db);
  assert.equal(summary(rows, "last_24h").usd_value, 7);
  assert.equal(summary(rows, "run_rate_usd_per_day_7d").usd_value, 2);
  assert.equal(summary(rows, "last_30d_total").usd_value, 21);
  assert.equal(runDaily(db).reduce((sum, row) => sum + row.usd, 0), 28);
});

test("a missing used rate excludes the whole row rather than publishing partial dollars", () => {
  const db = makeDb();
  insertPrice(db, { model: "partial", input: 5 });
  insertSession(db, { model: "partial", input: 1_000_000, output: 1_000_000 });
  insertUsage(db, { model: "partial", input: 1_000_000, output: 1_000_000 });
  assert.equal(runDaily(db)[0].usd, 0);
  assert.equal(summary(runDashboard(db), "last_24h").usd_value, 0);
  assert.equal(db.prepare(MODEL_SQL).get().list_usd, 0);
  for (const row of [runDaily(db)[0], summary(runDashboard(db), "last_24h"),
    db.prepare(MODEL_SQL).get()]) assert.equal(row.unpriced_tokens, 2_000_000);
});

test("empty and zero-token usage never claim full price coverage", () => {
  const db = makeDb();
  assert.equal(summary(runDashboard(db), "last_24h").priced_token_pct, null);
  assert.equal(runDaily(db).length, 0);
  assert.equal(db.prepare(MODEL_SQL).all().length, 0);
  insertSession(db, { model: "unknown" });
  insertUsage(db, { model: "unknown" });
  assert.equal(runDaily(db)[0].priced_pct, null);
  assert.equal(summary(runDashboard(db), "last_24h").priced_token_pct, null);
  assert.equal(db.prepare(MODEL_SQL).get().tokens, 0);
});

test("the authoritative dashboard reads the real v2 ledger trigger exactly once", () => {
  const db = makeDb();
  insertPrice(db, { model: "m", input: 5, output: 30, cacheRead: 0.5,
    cacheWrite: 6.25, inputIncludesCacheRead: 1 });
  const insert = db.prepare(`INSERT OR IGNORE INTO usage_report_ledger
    (receipt_key, usage_contract_version, usage_date, source, provider,
     model, build_channel, is_ci, responses, input_tokens, output_tokens,
     cache_read_input_tokens, cache_creation_input_tokens, total_tokens)
    VALUES (?, ?, date('now'), 'agent', 'test', 'm', 'stable', 0, 1,
      100000, 10000, 900000, 50000, 1060000)`);
  insert.run("response-v2", 2);
  insert.run("response-v2", 2);
  insert.run("response-v1", 1);
  const row = db.prepare(MODEL_SQL).get();
  assert.equal(row.responses, 1);
  assert.equal(row.tokens, 1_060_000);
  assert.equal(row.list_usd, 1.56);
});
