import test from "node:test";
import assert from "node:assert/strict";
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
  assert.match(UPSERT_PRICE_SQL, /WHERE excluded\.price_kind <> 'unpriced' OR model_prices\.price_kind = 'unpriced'/);
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
  assert.deepEqual(result, { labels: 2, priced: 1 });
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
