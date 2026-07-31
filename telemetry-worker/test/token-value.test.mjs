// Tests for the token-value dashboard's pricing math.
//
// These run token-value.sql against an in-memory SQLite database with a tiny
// synthetic fixture, so the accounting rules are checked by the same SQL the
// dashboard runs rather than by a reimplementation. The rule that matters most:
// OpenAI-compatible providers report cached tokens as a SUBSET of input tokens
// while Anthropic reports them as a disjoint bucket, so pricing input without
// subtracting cache reads overcharges OpenAI traffic by roughly 10x. Cache
// reads are ~85% of all tokens in production, so that single mistake would
// dominate the headline number.
//
// Run with: node --test test/
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
     VALUES ('session_end', datetime('now', '-2 hours'), ?, ?, ?, ?, ?, ?, ?, ?)`,
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
  // gpt-5.6-sol list rates. input_tokens includes the cached portion.
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
  // claude-opus-5 list rates; Anthropic reports cache reads separately.
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
  // Same effective usage as the OpenAI case above, so the same $1.25 shape:
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
