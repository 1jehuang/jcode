import test from "node:test";
import assert from "node:assert/strict";
import { buildModelPriceReport } from "../scripts/model-price-report.mjs";

test("unmatched coverage includes every provider for the same label", () => {
  const report = buildModelPriceReport([
    { model: "Coding", provider: "OpenAI", tokens: 100, sessions: 1 },
    { model: "Coding", provider: "OpenRouter", tokens: 300, sessions: 2 },
    { model: "known", provider: "Claude", tokens: 600, sessions: 3 },
  ], new Map([["known", { kind: "catalog", input: 5 }]]));
  assert.equal(report.matched_token_pct, 60);
  assert.equal(report.unpriced_reported_tokens, 400);
  assert.deepEqual(report.unpriced, [{
    model: "Coding", reported_tokens: 400, sessions: 3,
    providers: ["OpenAI", "OpenRouter"],
  }]);
});

test("zero price is matched, missing price stays unknown", () => {
  const report = buildModelPriceReport([
    { model: "free", tokens: 50 }, { model: "missing", tokens: 25 },
    { model: "broken", tokens: 25 },
  ], new Map([
    ["free", { kind: "free", input: 0 }],
    ["missing", { kind: "unpriced", input: null }],
    ["broken", { kind: "catalog", input: null }],
  ]));
  assert.equal(report.matched_token_pct, 50);
  assert.equal(report.unpriced.length, 2);
});

test("empty usage is unknown coverage, not 100 percent", () => {
  const report = buildModelPriceReport([], new Map());
  assert.equal(report.matched_token_pct, null);
  assert.deepEqual(report.unpriced, []);
});

test("unmatched labels are ranked by combined volume and serialize cleanly", () => {
  const report = buildModelPriceReport([
    { model: "small", tokens: 5 }, { model: "large", provider: "Claude", tokens: 10 },
    { model: "large", provider: "Claude", tokens: 10 },
  ], new Map());
  assert.deepEqual(report.unpriced.map((r) => r.model), ["large", "small"]);
  assert.deepEqual(JSON.parse(JSON.stringify(report)), report);
  assert.deepEqual(report.unpriced[0].providers, ["Claude"]);
});

test("invalid counts fail instead of publishing false coverage", () => {
  for (const tokens of [-1, NaN, Infinity, "invalid"]) {
    assert.throws(() => buildModelPriceReport([{ model: "bad", tokens }], new Map()));
  }
});
