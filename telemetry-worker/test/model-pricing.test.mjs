import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import {
  UPSERT_PRICE_SQL, buildPriceIndex, normalizeLabel, resolvePriceRows, syncModelPrices,
} from "../src/model-pricing.js";

const opus = { input: 4, output: 20, cache_read: 0.2, cache_write: 5 };
const catalog = {
  anthropic: { models: { "claude-opus-5-5": { cost: opus } } },
  zai: { models: { "glm-5.3": { cost: { input: 1, output: 3 } } } },
  google: { models: { "gemini-3.8-flash": { cost: { input: 0.3, output: 2.5 } } } },
};

function priceOf(label) {
  return resolvePriceRows([{ model: label, provider: "x" }], buildPriceIndex(catalog)).get(label);
}

test("Opus 5.5 labels from every observed gateway resolve to the Opus 5.5 price", () => {
  for (const label of [
    "claude-opus-5-5", "claude-opus-5.5", "cc/claude-opus-5-5(high)",
    "cc/claude-opus-5-5(xhigh)", "cc/claude-opus-5-5(max)", "anthropic-oauth:opus-5-5",
    "opus-5.5", "openrouter:cc/claude-opus-5-5", "claude-opus-5-5-thinking",
    "claude-opus-5-5@Anthropic", "claude-opus-5-5[1m]",
  ]) {
    const row = priceOf(label);
    assert.equal(row.kind, "catalog", label);
    assert.equal(row.input, 4, label);
    assert.equal(row.sourceModel, "claude-opus-5-5", label);
  }
});

test("route prefixes, @cf namespaces and tiered variants resolve", () => {
  assert.equal(priceOf("zai-api:glm-5.3").sourceModel, "glm-5.3");
  assert.equal(priceOf("@cf/zai-org/glm-5.3").sourceModel, "glm-5.3");
  assert.equal(priceOf("ag/gemini-3.8-flash-tiered").sourceModel, "gemini-3.8-flash");
});

test("opaque router aliases stay unpriced rather than guessed", () => {
  for (const label of ["high", "coder", "tommy-jcode:latest", "stealth/space-bunny-alpha"]) {
    assert.equal(priceOf(label).kind, "unpriced", label);
  }
  assert.ok(!normalizeLabel("tommy-jcode:latest").includes("latest"));
});

test("upsert never downgrades a known price to unpriced", () => {
  assert.match(UPSERT_PRICE_SQL, /WHERE \(CASE excluded\.price_kind/);
});

test("upsert ranks catalog > family_estimate > unpriced in real SQLite", () => {
  const db = new DatabaseSync(":memory:");
  db.exec(readFileSync(new URL("../migrations/0023_model_prices.sql", import.meta.url), "utf8"));
  const put = (kind, input) => db.prepare(UPSERT_PRICE_SQL).run("m", null, null, input, null, null, null, 0, kind);
  const get = () => db.prepare("SELECT price_kind, input_usd_per_mtok AS i FROM model_prices WHERE model='m'").get();
  put("unpriced", null);
  put("family_estimate", 5);
  assert.deepEqual({ ...get() }, { price_kind: "family_estimate", i: 5 });
  put("unpriced", null);
  assert.equal(get().i, 5, "miss does not erase estimate");
  put("catalog", 4);
  assert.deepEqual({ ...get() }, { price_kind: "catalog", i: 4 });
  put("family_estimate", 9);
  assert.equal(get().i, 4, "estimate does not replace exact price");
  put("catalog", 3);
  assert.equal(get().i, 3, "catalog price changes still apply");
});

test("a new release missing from the catalog is priced from its family", () => {
  const cat = {
    anthropic: { models: {
      "claude-opus-4-5": { cost: { input: 5, output: 25 } },
      "claude-opus-5-5": { cost: opus },
      "claude-sonnet-5-5": { cost: { input: 2, output: 10 } },
    } },
    openai: { models: { "gpt-6.1-sol": { cost: { input: 2, output: 10 } } } },
  };
  const index = buildPriceIndex(cat);
  const rows = resolvePriceRows([
    { model: "claude-opus-6" }, { model: "cc/claude-opus-6-1(high)" },
    { model: "gpt-6.2-sol" }, { model: "coder" }, { model: "glm-9" }, { model: "claude-opus-4-1" },
  ], index);
  assert.equal(rows.get("claude-opus-4-1").kind, "unpriced", "older releases do not borrow newer prices");
  assert.equal(rows.get("claude-opus-6").kind, "family_estimate");
  assert.equal(rows.get("claude-opus-6").sourceModel, "claude-opus-5-5", "newest family member");
  assert.equal(rows.get("cc/claude-opus-6-1(high)").input, 4);
  assert.equal(rows.get("gpt-6.2-sol").sourceModel, "gpt-6.1-sol");
  assert.equal(rows.get("coder").kind, "unpriced", "unversioned aliases are not guessed");
  assert.equal(rows.get("glm-9").kind, "unpriced", "no family, no guess");
});

test("syncModelPrices prices recent labels and batches the upserts", async () => {
  const big = { anthropic: { models: { ...catalog.anthropic.models } } };
  for (let i = 0; i < 150; i++) big.anthropic.models[`filler-${i}`] = { cost: { input: 1 } };
  const batches = [];
  let boundDays;
  const env = {
    DB: {
      prepare(sql) {
        return {
          sql,
          bind(...args) {
            if (sql.includes("FROM events")) boundDays = args[0];
            if (sql.includes("daily_model_usage")) {
              return { all: async () => ({ results: [
                { model: "claude-opus-5-5", provider: "anthropic", tokens: 7 },
              ] }) };
            }
            return { sql, args, all: async () => ({ results: [
              { model: "cc/claude-opus-5-5(high)", provider: "openrouter", tokens: 10 },
              { model: "mystery", provider: "openrouter", tokens: 5 },
            ] }) };
          },
        };
      },
      async batch(stmts) { batches.push(stmts); return []; },
    },
  };
  const result = await syncModelPrices(env, {
    fetchImpl: async () => ({ ok: true, json: async () => big }),
  });
  assert.equal(boundDays, "-2 days");
  assert.deepEqual(result, { labels: 3, priced: 2 });
  const opusRow = batches.flat().find((s) => s.args[0] === "cc/claude-opus-5-5(high)");
  assert.equal(opusRow.args[3], 4);
  assert.equal(opusRow.args[7], 1, "openrouter reports cache reads inside input");
});

test("syncModelPrices refuses a truncated catalog", async () => {
  await assert.rejects(
    syncModelPrices({ DB: {} }, { fetchImpl: async () => ({ ok: true, json: async () => catalog }) }),
    /catalog too small/,
  );
});
