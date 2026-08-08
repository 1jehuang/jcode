#!/usr/bin/env node
// Sync list prices into the D1 `model_prices` table (migration 0023).
//
// Why this exists: the telemetry DB records per-session token counts and the
// model/provider labels, but no prices, so the dashboards could only show raw
// token volume. This script pulls the free models.dev catalog (no auth, same
// source the CLI uses in crates/jcode-base/src/model_pricing.rs) and writes one
// row per model label actually observed in telemetry, so the token-value
// dashboard can price each model at its own rate instead of applying one
// blended guess to everything.
//
// Usage:
//   node scripts/sync-model-prices.mjs            # apply to remote D1
//   node scripts/sync-model-prices.mjs --dry-run  # print the SQL and a report
//   node scripts/sync-model-prices.mjs --days=90  # widen the observed window
//
// Safe to re-run: it is an upsert keyed on the telemetry model label, and it
// never deletes rows.

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const CATALOG_URL = "https://models.dev/api.json";
const DB = "jcode-telemetry";

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const daysArg = args.find((a) => a.startsWith("--days="));
const days = daysArg ? Number.parseInt(daysArg.slice("--days=".length), 10) : 30;
if (!Number.isFinite(days) || days <= 0) {
  console.error(`invalid --days value: ${daysArg}`);
  process.exit(1);
}

// Providers whose reported input token count already includes cache reads.
// OpenAI-compatible APIs report `prompt_tokens_details.cached_tokens` as a
// subset of `prompt_tokens`, so pricing input and cache-read separately would
// bill the cached portion twice. Anthropic's Messages API reports
// `input_tokens` and `cache_read_input_tokens` as disjoint buckets.
// Keep in sync with jcode-compaction-core's estimate_compaction_tokens note.
const CACHE_INCLUSIVE_PROVIDERS = new Set([
  "openai",
  "openrouter",
  "copilot",
  "gemini",
  "google",
  "xai",
  "groq",
  "cerebras",
  "deepseek",
  "moonshot",
  "zai",
  "minimax",
  "mistral",
  "ollama",
  "openai-compatible",
]);

// jcode's own internal route labels, which are real priced traffic but are not
// models.dev ids. Mapped explicitly so they do not land in the unpriced bucket.
const LABEL_ALIASES = new Map([
  // The auto code-review pass runs on the Codex model family.
  ["codex-auto-review", "gpt-5.3-codex"],
  ["codex-auto-review-spark", "gpt-5.3-codex-spark"],
]);

// Model labels that are not real priced models: local/mock/routing sentinels.
// These are recorded as price_kind='free' so the dashboard can separate
// "genuinely $0" from "we failed to find a price".
const FREE_LABEL_PATTERNS = [
  /^mock$/i,
  /^unknown$/i,
  /^default$/i,
  /^auto(\/.*)?$/i,
  /^pool\d*$/i,
  /^free$/i,
  /^openrouter\/free$/i,
  /:free$/i,
  /-free$/i,
  /^ollama\//i,
  /:cloud$/i,
  /^qwen[\d.]*(-coder)?:\d+b$/i,
  /^qwen\d[\d.]*:\d+b$/i,
  /-coding-plan\//i,
  /-token-plan/i,
];

// Batched writes go through `--file` (wrangler's import path), which is the
// only way to apply hundreds of statements in one round trip.
function runWranglerFile(sqlText, label) {
  const dir = mkdtempSync(join(tmpdir(), "model-prices-"));
  const file = join(dir, `${label}.sql`);
  writeFileSync(file, sqlText);
  const out = execFileSync(
    "npx",
    ["wrangler", "d1", "execute", DB, "--remote", "--json", `--file=${file}`],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  return out;
}

// Reads must use `--command`: the `--file` import path reports only
// rows-read/rows-written counters and discards the actual result set.
function queryRows(sql) {
  const raw = execFileSync(
    "npx",
    ["wrangler", "d1", "execute", DB, "--remote", "--json", "--command", sql],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  const start = raw.indexOf("[");
  if (start < 0) throw new Error(`unexpected wrangler output:\n${raw}`);
  const parsed = JSON.parse(raw.slice(start));
  return parsed[0]?.results ?? [];
}

// Normalize a telemetry model label toward a models.dev model id. Users reach
// the same model through many gateways that prefix or rename it.
function normalizeLabel(label) {
  let s = label.trim();
  const alias = LABEL_ALIASES.get(s.toLowerCase());
  if (alias) return [alias];
  // Strip jcode's own suffixes/markers.
  s = s.replace(/\[1m\]$/i, "").replace(/\[web\]$/i, "");
  // Strip a trailing `@Provider` disambiguator (`...-4-8@Anthropic`) and
  // reasoning-effort suffixes (`-xhigh`, `-high`), which never change price.
  s = s.replace(/@[^@]+$/, "");
  s = s.replace(/-(xhigh|x-high|max-effort)$/i, "");
  // Strip gateway/route prefixes: `cc/`, `oc/`, `cx/`, `kr/`, `ag/`, `nvidia/z-ai/`.
  const parts = s.split("/");
  const candidates = new Set([s]);
  if (parts.length > 1) {
    candidates.add(parts[parts.length - 1]);
    candidates.add(parts.slice(1).join("/"));
  }
  // Effort suffixes can also sit on the gateway-stripped form.
  for (const c of [...candidates]) {
    candidates.add(c.replace(/-(xhigh|high|medium|low|none|minimal)$/i, ""));
  }
  // Anthropic dated ids: claude-opus-4-5-20251101 -> claude-opus-4-5.
  for (const c of [...candidates]) {
    const undated = c.replace(/-\d{8}$/, "");
    if (undated !== c) candidates.add(undated);
    // Dotted vs dashed Anthropic spellings: claude-sonnet-4.6 <-> 4-6.
    if (/^claude-/.test(undated)) {
      candidates.add(undated.replace(/(\d)\.(\d)/g, "$1-$2"));
      candidates.add(undated.replace(/(\d)-(\d)(?!\d)/g, "$1.$2"));
    }
    // Thinking/highspeed variants price the same as the base model.
    candidates.add(undated.replace(/-(thinking|highspeed|high|medium|low)$/i, ""));
  }
  return [...candidates].filter(Boolean);
}

function isFreeLabel(label) {
  return FREE_LABEL_PATTERNS.some((re) => re.test(label));
}

function buildPriceIndex(catalog) {
  // model id (lowercased) -> [{provider, cost}], provider-preference sorted.
  const index = new Map();
  const preference = [
    "anthropic",
    "openai",
    "google",
    "xai",
    "deepseek",
    "zhipuai",
    "moonshotai",
    "minimax",
    "openrouter",
  ];
  for (const [providerId, provider] of Object.entries(catalog)) {
    for (const [modelId, model] of Object.entries(provider.models ?? {})) {
      const cost = model.cost;
      if (!cost || typeof cost.input !== "number") continue;
      const key = modelId.toLowerCase();
      if (!index.has(key)) index.set(key, []);
      index.get(key).push({ provider: providerId, cost });
    }
  }
  for (const entries of index.values()) {
    entries.sort((a, b) => {
      const ai = preference.indexOf(a.provider);
      const bi = preference.indexOf(b.provider);
      // Known first-party providers first, then the cheapest-listed as a proxy
      // for "most likely the real rate" rather than an arbitrary map order.
      if (ai !== bi) return (ai < 0 ? 999 : ai) - (bi < 0 ? 999 : bi);
      return a.cost.input - b.cost.input;
    });
  }
  return index;
}

function sqlStr(value) {
  if (value === null || value === undefined) return "NULL";
  return `'${String(value).replace(/'/g, "''")}'`;
}

function sqlNum(value) {
  return value === null || value === undefined ? "NULL" : String(value);
}

async function main() {
  console.log(`Fetching ${CATALOG_URL} ...`);
  const response = await fetch(CATALOG_URL);
  if (!response.ok) throw new Error(`models.dev returned ${response.status}`);
  const catalog = await response.json();
  const index = buildPriceIndex(catalog);
  console.log(`Indexed ${index.size} priced model ids.`);

  console.log(`Reading model labels observed in the last ${days} days ...`);
  const observed = queryRows(
    `SELECT model_end AS model, provider_end AS provider, COUNT(*) AS sessions,
            SUM(input_tokens + output_tokens + cache_read_input_tokens
                + cache_creation_input_tokens) AS tokens
     FROM events
     WHERE event = 'session_end'
       AND created_at >= datetime('now', '-${days} days')
       AND model_end IS NOT NULL
     GROUP BY model_end, provider_end
     ORDER BY tokens DESC;`,
  );
  console.log(`Found ${observed.length} model/provider label pairs.`);

  const rows = new Map();
  const unpriced = [];
  for (const row of observed) {
    const label = row.model;
    if (rows.has(label)) continue;
    const providerKey = String(row.provider ?? "").toLowerCase();
    const cacheInclusive = CACHE_INCLUSIVE_PROVIDERS.has(providerKey) ? 1 : 0;

    if (isFreeLabel(label)) {
      rows.set(label, {
        model: label,
        sourceModel: null,
        sourceProvider: null,
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        cacheInclusive,
        kind: "free",
      });
      continue;
    }

    let match = null;
    for (const candidate of normalizeLabel(label)) {
      const entries = index.get(candidate.toLowerCase());
      if (entries?.length) {
        match = { candidate, ...entries[0] };
        break;
      }
    }

    if (!match) {
      unpriced.push({ label, tokens: row.tokens ?? 0 });
      rows.set(label, {
        model: label,
        sourceModel: null,
        sourceProvider: null,
        input: null,
        output: null,
        cacheRead: null,
        cacheWrite: null,
        cacheInclusive,
        kind: "unpriced",
      });
      continue;
    }

    const cost = match.cost;
    rows.set(label, {
      model: label,
      sourceModel: match.candidate,
      sourceProvider: match.provider,
      input: cost.input ?? null,
      output: cost.output ?? null,
      // Anthropic-style caching: a cache read costs 0.1x input when the
      // catalog omits an explicit rate.
      cacheRead: cost.cache_read ?? (cost.input != null ? cost.input * 0.1 : null),
      cacheWrite: cost.cache_write ?? (cost.input != null ? cost.input * 1.25 : null),
      cacheInclusive,
      kind: "catalog",
    });
  }

  const statements = [...rows.values()].map(
    (r) =>
      `INSERT INTO model_prices (model, source_model, source_provider, input_usd_per_mtok, output_usd_per_mtok, cache_read_usd_per_mtok, cache_write_usd_per_mtok, input_includes_cache_read, price_kind, updated_at) VALUES (${sqlStr(r.model)}, ${sqlStr(r.sourceModel)}, ${sqlStr(r.sourceProvider)}, ${sqlNum(r.input)}, ${sqlNum(r.output)}, ${sqlNum(r.cacheRead)}, ${sqlNum(r.cacheWrite)}, ${r.cacheInclusive}, ${sqlStr(r.kind)}, datetime('now')) ON CONFLICT(model) DO UPDATE SET source_model=excluded.source_model, source_provider=excluded.source_provider, input_usd_per_mtok=excluded.input_usd_per_mtok, output_usd_per_mtok=excluded.output_usd_per_mtok, cache_read_usd_per_mtok=excluded.cache_read_usd_per_mtok, cache_write_usd_per_mtok=excluded.cache_write_usd_per_mtok, input_includes_cache_read=excluded.input_includes_cache_read, price_kind=excluded.price_kind, updated_at=datetime('now');`,
  );

  const totalTokens = observed.reduce((sum, r) => sum + Number(r.tokens ?? 0), 0);
  const unpricedTokens = unpriced.reduce((sum, r) => sum + Number(r.tokens ?? 0), 0);
  const coverage = totalTokens ? (1 - unpricedTokens / totalTokens) * 100 : 100;
  console.log(
    `Priced ${rows.size - unpriced.length}/${rows.size} labels; ` +
      `token coverage ${coverage.toFixed(2)}%.`,
  );
  if (unpriced.length) {
    console.log("Top unpriced labels by tokens:");
    for (const u of unpriced.sort((a, b) => Number(b.tokens) - Number(a.tokens)).slice(0, 15)) {
      console.log(`  ${u.label}  ${Number(u.tokens).toLocaleString()} tokens`);
    }
  }

  if (dryRun) {
    console.log(`\n--dry-run: ${statements.length} statements not applied.`);
    console.log(statements.slice(0, 3).join("\n"));
    return;
  }

  console.log(`Applying ${statements.length} upserts to ${DB} ...`);
  runWranglerFile(statements.join("\n"), "upsert");
  console.log("Done. Run `npm run token-value` to see the priced dashboard.");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
