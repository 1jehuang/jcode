import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import {
  UPSERT_PRICE_SQL, buildPriceIndex, normalizeLabel, resolvePriceRows, syncModelPrices, usageLabelsSql,
} from "../src/model-pricing.js";
import worker, { syncModelPricesWithRetry } from "../src/worker.js";

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

test("unknown and subscription routes do not masquerade as free inference", () => {
  for (const label of [
    "unknown", "default", "auto", "auto/router", "pool", "pool12",
    "mystery:cloud", "ollama/mystery:cloud", "vendor-coding-plan/mystery",
    "mystery-token-plan",
  ]) {
    const row = priceOf(label);
    assert.equal(row.kind, "unpriced", label);
    assert.equal(row.input, null, label);
  }
  assert.equal(priceOf("vendor-coding-plan/claude-opus-5-5").input, 4,
    "subscription-backed inference retains its underlying model's list price");
  for (const label of ["mock", "free", "openrouter/free", "model:free", "ollama/local-model"]) {
    assert.equal(priceOf(label).kind, "free", label);
  }
});

test("upsert repairs obsolete free classifications without erasing catalog rates", () => {
  const db = new DatabaseSync(":memory:");
  db.exec(readFileSync(new URL("../migrations/0023_model_prices.sql", import.meta.url), "utf8"));
  const put = (kind, input) => db.prepare(UPSERT_PRICE_SQL)
    .run("some-alias", null, null, input, null, null, null, 0, kind);
  const get = () => db.prepare("SELECT price_kind, input_usd_per_mtok AS i FROM model_prices WHERE model='some-alias'").get();
  put("free", 0);
  put("unpriced", null);
  assert.deepEqual({ ...get() }, { price_kind: "unpriced", i: null });
  put("catalog", 4);
  put("unpriced", null);
  assert.deepEqual({ ...get() }, { price_kind: "catalog", i: 4 });
  db.close();
});

test("upsert clears a stale price left on a router placeholder, and only there", () => {
  const db = new DatabaseSync(":memory:");
  db.exec(readFileSync(new URL("../migrations/0023_model_prices.sql", import.meta.url), "utf8"));
  const put = (model, kind, input) => db.prepare(UPSERT_PRICE_SQL)
    .run(model, null, null, input, null, null, null, 0, kind);
  const kind = (model) => db.prepare("SELECT price_kind AS k FROM model_prices WHERE model=?").get(model).k;
  const labels = ["auto", "Default", "unknown", "router", "pool", "pool12", "pooled-model", "pool1x", "auto-router-9"];
  for (const label of labels) {
    put(label, "catalog", 0);
    put(label, "unpriced", null);
    // The SQL guard must agree with what the resolver refuses to price.
    const placeholder = resolvePriceRows([{ model: label }],
      buildPriceIndex({ gw: { models: { [label]: { cost: { input: 1, output: 2 } } } } })).get(label).kind === "unpriced";
    assert.equal(kind(label), placeholder ? "unpriced" : "catalog", label);
  }
  assert.equal(kind("auto"), "unpriced");
  assert.equal(kind("pooled-model"), "catalog");
  db.close();
});

test("gateway respellings of a known release are priced as estimates", () => {
  const index = buildPriceIndex({
    anthropic: { models: { "claude-opus-5-5": { cost: opus }, "claude-sonnet-5-5": { cost: { input: 2, output: 10 } } } },
    openrouter: { models: {
      "deepseek/deepseek-v4.1-flash-1017": { cost: { input: 0.04, output: 0.5 } },
      "deepseek/deepseek-v4.1-flash": { cost: { input: 0.2, output: 1 } },
      "qwen/qwen3.8-27b": { cost: { input: 0.15, output: 1.9 } },
      "stepfun/step-5-preview": { cost: { input: 1, output: 2.7 } },
      "openai/gpt-5": { cost: { input: 1.25, output: 10 } },
    } },
  });
  const of = (label) => resolvePriceRows([{ model: label, provider: "openrouter" }], index).get(label);
  const cases = {
    "@preset/deepseek-4-1-flash-1017-q8": 0.04,
    "coa/DeepSeek V4.1 Flash 1017": 0.04,
    "deepseek-v-4.1-flash-1017": 0.04,
    "antigravity-opus5.5": 4,
    "Claude Sonnet 5.5": 2,
    "qwen38-27b-unc": 0.15,
    "step-5": 1,
  };
  for (const [label, input] of Object.entries(cases)) {
    assert.equal(of(label).kind, "family_estimate", label);
    assert.equal(of(label).input, input, label);
  }
  // No model name, a different release, or a router placeholder: no guess.
  for (const label of ["DeepSeek-flash-combo", "combo-ds-v4.1-flash", "wrap-gpt-5-5-x", "high", "tier2", "pool12", "1"]) {
    assert.equal(of(label).kind, "unpriced", label);
  }
});

test("models that can only be running locally count as free, not unpriced", () => {
  const index = buildPriceIndex({
    ...catalog,
    hosted: { models: { "gemma4:12b": { cost: { input: 0.1, output: 0.3 } } } },
  });
  const kind = (label) => resolvePriceRows([{ model: label }], index).get(label).kind;
  for (const label of [
    "Qwen 3.8 FN UD-IQ3_XXS (Basalt)", "LFM3-Omni-2B-MLX-8bit", "swift15-iq2xs.gguf", "flint:40b",
    "hf.co/xtools-at/LFM3-1.7B-GGUF:Q8_0", "/Users/someone/models/LFM3-1.7B-MLX-4bit",
    "ornith-ai/Ornith-1.5-35B-A3B-FP8", "ripple65b-nvfp4", "gemma4:e4b",
  ]) {
    assert.equal(kind(label), "free", label);
  }
  // A hosted listing with the same id keeps its price.
  assert.equal(kind("gemma4:12b"), "catalog");
  // Not proof of local hardware, so these stay visible as unpriced.
  for (const label of ["hermes-qwen:latest", "uncensored", "gpt-oss:120b-cloud", "mystery:cloud", "coder"]) {
    assert.equal(kind(label), "unpriced", label);
  }
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
  const usageTables = [];
  let boundDays;
  const env = {
    DB: {
      prepare(sql) {
        return {
          sql,
          bind(...args) {
            if (sql.includes("FROM events")) boundDays = args[0];
            if (sql.includes("daily_model_usage")) {
              const canonical = sql.includes("daily_model_usage_v2");
              usageTables.push(canonical ? "v2" : "v1");
              return { all: async () => ({ results: [
                { model: canonical ? "v2-only/claude-opus-5-5" : "claude-opus-5-5", provider: "anthropic", tokens: 7 },
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
  assert.equal(boundDays, "-3 days");
  assert.deepEqual(result, { labels: 4, priced: 3 });
  assert.deepEqual(usageTables, ["v1", "v2"]);
  const opusRow = batches.flat().find((s) => s.args[0] === "cc/claude-opus-5-5(high)");
  assert.equal(opusRow.args[3], 4);
  assert.equal(opusRow.args[7], 1, "openrouter reports cache reads inside input");
});

test("usage price discovery includes delayed reports updated today", () => {
  const db = new DatabaseSync(":memory:");
  db.exec(readFileSync(new URL("../migrations/0027_daily_model_usage.sql", import.meta.url), "utf8"));
  db.prepare(`INSERT INTO daily_model_usage
    (usage_date, source, provider, model, responses, total_tokens)
    VALUES (date('now','-6 days'), 'sidecar', 'anthropic', 'new-model', 1, 100)`)
    .run();
  const rows = db.prepare(usageLabelsSql()).all("-2 days", "-2 days");
  assert.equal(rows.length, 1);
  assert.equal(rows[0].model, "new-model");
  db.close();
});

test("price discovery never hides an overloaded usage store", async () => {
  const big = { anthropic: { models: {} } };
  for (let i = 0; i < 150; i++) big.anthropic.models[`filler-${i}`] = { cost: { input: 1 } };
  const env = { DB: { prepare(sql) { return { bind() { return { async all() {
    if (sql.includes("daily_model_usage")) throw new Error("D1 DB is overloaded");
    return { results: [] };
  } }; } }; } } };
  await assert.rejects(syncModelPrices(env, {
    fetchImpl: async () => ({ ok: true, json: async () => big }),
  }), /D1 DB is overloaded/);
});

test("syncModelPrices refuses a truncated catalog", async () => {
  await assert.rejects(
    syncModelPrices({ DB: {} }, { fetchImpl: async () => ({ ok: true, json: async () => catalog }) }),
    /catalog too small/,
  );
});

// Regressions for the 2026-11 priced-share drop: recognisable models were
// landing unpriced, or priced at $0 from subscription-plan catalog entries.
const gaps = {
  anthropic: { models: { "claude-opus-5-5": { cost: opus }, "claude-fable-5-2": { cost: { input: 8, output: 40 } } } },
  "volcengine-coding-plan": { models: {
    "deepseek-v4.1-flash": { cost: { input: 0, output: 0 } },
    "glm-5.3": { cost: { input: 0, output: 0 } },
  } },
  engy: { models: { "deepseek-v4.1-flash": { cost: { input: 0.03, output: 0.12 } } } },
  zai: { models: { "glm-5.3": { cost: { input: 1.4, output: 4.4 } } } },
  openai: { models: { "gpt-6.1-sol": { cost: { input: 2, output: 10 } }, "gpt-5": { cost: { input: 1, output: 8 } } } },
  vercel: { models: { "meituan/longcat-2.5": { cost: { input: 0.3, output: 1.2 } } } },
  "opencode-go": { models: { "qwen4-max": { cost: { input: 1.5, output: 4.5 } } } },
  jzstoken: { models: { "claude-opus-5.5": { cost: { input: 1.17, output: 5.84 } } } },
  "token-plan": { models: { "plan-only-9": { cost: { input: 0, output: 0 } } } },
};
const gapPrice = (label) =>
  resolvePriceRows([{ model: label, provider: "openrouter" }], buildPriceIndex(gaps)).get(label);

test("a subscription plan's $0 listing never shadows a real list price", () => {
  assert.equal(gapPrice("deepseek-v4.1-flash").input, 0.03);
  assert.equal(gapPrice("glm-5.3").sourceProvider, "zai");
  assert.equal(gapPrice("plan-only-9").input, 0, "a model listed only at $0 keeps that listing");
});

test("ollama cloud tags price as the base model", () => {
  for (const label of ["deepseek-v4.1-flash:cloud", "deepseek-v4.1-flash:1017-cloud", "glm-5.3:cloud"]) {
    const row = gapPrice(label);
    assert.equal(row.kind, "catalog", label);
    assert.ok(row.input > 0, label);
  }
  assert.equal(gapPrice("mystery:cloud").kind, "unpriced");
});

test("fast, preview and vendor-prefixed catalog ids resolve", () => {
  assert.equal(gapPrice("claude-fable-5-2-fast").input, 8);
  assert.equal(gapPrice("qwen4-max-preview").sourceModel, "qwen4-max");
  assert.equal(gapPrice("LongCat-2.5").input, 0.3, "bare label matches `meituan/longcat-2.5`");
});

test("a first-party price beats a reseller's literal spelling", () => {
  const row = gapPrice("claude-opus-5.5");
  assert.equal(row.sourceProvider, "anthropic");
  assert.equal(row.input, 4);
});

test("gateway-wrapped model ids are estimated, opaque aliases are not", () => {
  for (const [label, input] of [
    ["maas-glm-5.3-zhipu-codeagent", 1.4], ["aixdev-gpt-6.1-sol", 2],
    ["CodeMate/DeepSeek-V4.1-Flash(BaiLian-MaaS)", 0.03], ["kr/claude-opus-5.5-agentic", 4],
  ]) {
    const row = gapPrice(label);
    assert.equal(row.kind, "family_estimate", label);
    assert.equal(row.input, input, label);
  }
  for (const label of ["high", "coder", "large-stack", "work-agent", "brain", "tier2", "gpt-5-5", "xgpt-5"]) {
    assert.equal(gapPrice(label).kind, "unpriced", label);
  }
});

test("a gateway's catalog id named like a router placeholder prices nothing", () => {
  const index = buildPriceIndex({ morph: { models: { auto: { cost: { input: 0.85, output: 1.55 } } } } });
  assert.equal(resolvePriceRows([{ model: "auto" }], index).get("auto").kind, "unpriced");
});

function pricingEnv({ failures = 0, log = [] } = {}) {
  let remaining = failures;
  return { log, DB: {
    prepare(sql) {
      const kind = /INSERT INTO model_prices/.test(sql) ? "price"
        : /DELETE FROM usage_report_ledger/.test(sql) ? "ledger" : "other";
      const stmt = {
        kind,
        bind() { return stmt; },
        async all() {
          if (/FROM events/.test(sql) && /model_end AS model/.test(sql)) {
            if (remaining-- > 0) throw new Error("D1 DB is overloaded. [code: 7429]");
            return { results: [{ model: "claude-opus-5-5", provider: "Claude", tokens: 9 }] };
          }
          return { results: [] };
        },
        async run() { log.push(kind); return { meta: { changes: 0 } }; },
        async first() { return null; },
      };
      return stmt;
    },
    async batch(stmts) { for (const s of stmts) log.push(s.kind); return []; },
  } };
}
const bigCatalog = () => {
  const big = { anthropic: { models: { ...catalog.anthropic.models } } };
  for (let i = 0; i < 150; i++) big.anthropic.models[`filler-${i}`] = { cost: { input: 1 } };
  return { ok: true, json: async () => big };
};

test("nightly cron prices labels before the ledger retention loop", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => bigCatalog();
  try {
    const env = pricingEnv();
    const jobs = [];
    await worker.scheduled({ cron: "17 4 * * *" }, env, { waitUntil(p) { jobs.push(p); } });
    await Promise.all(jobs);
    const price = env.log.indexOf("price");
    const ledger = env.log.indexOf("ledger");
    assert.ok(price >= 0, "prices were written");
    assert.ok(ledger > price, `pricing ran first: ${env.log.slice(0, 6)}`);
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("price sync retries once after a D1 overload and never throws", async () => {
  const once = pricingEnv({ failures: 1 });
  const result = await syncModelPricesWithRetry(once, { cooldownMs: 1, fetchImpl: async () => bigCatalog() });
  assert.deepEqual(result, { labels: 1, priced: 1 });
  const twice = pricingEnv({ failures: 2 });
  assert.equal(await syncModelPricesWithRetry(twice, { cooldownMs: 1, fetchImpl: async () => bigCatalog() }), null);
  assert.equal(twice.log.includes("price"), false);
  let calls = 0;
  assert.equal(await syncModelPricesWithRetry({ DB: {} }, { cooldownMs: 1, fetchImpl: async () => { calls++; return { ok: false, status: 503 }; } }), null);
  assert.equal(calls, 1, "a catalog outage is not retried");
});
