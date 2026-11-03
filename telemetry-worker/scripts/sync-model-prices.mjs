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
//   node scripts/sync-model-prices.mjs --dry-run --report-json=unpriced.json
//
// Safe to re-run: it is an upsert keyed on the telemetry model label, and it
// never deletes rows.

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildModelPriceReport } from "./model-price-report.mjs";
import {
  CATALOG_URL, PRICE_UPSERT_GUARD_SQL, buildPriceIndex, observedLabelsSql, usageLabelsSql, resolvePriceRows,
} from "../src/model-pricing.js";
const DB = "jcode-telemetry";

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const reportArg = args.find((a) => a.startsWith("--report-json="));
const reportPath = reportArg?.slice("--report-json=".length);
if (reportArg && !reportPath) {
  console.error("--report-json requires a file path");
  process.exit(1);
}
const daysArg = args.find((a) => a.startsWith("--days="));
const days = daysArg ? Number.parseInt(daysArg.slice("--days=".length), 10) : 30;
if (!Number.isFinite(days) || days <= 0) {
  console.error(`invalid --days value: ${daysArg}`);
  process.exit(1);
}

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
    observedLabelsSql().replace("?", `'-${days} days'`) + ";",
  );
  for (const canonical of [false, true]) {
    try {
      observed.push(...queryRows(
        usageLabelsSql(canonical).replaceAll("?", `'-${days} days'`) + ";",
      ));
    } catch (err) {
      // Missing tables are expected during rolling migrations, not auth errors
      // or overload. Never claim a successful refresh after an operational fault.
      const detail = [err?.stdout, err?.stderr, err?.message].filter(Boolean).join("\n");
      if (!/no such table/i.test(detail)) throw err;
      console.warn(`${canonical ? "daily_model_usage_v2" : "daily_model_usage"} unavailable; apply telemetry migrations.`);
    }
  }
  console.log(`Found ${observed.length} model/provider label pairs.`);

  const rows = resolvePriceRows(observed, index);
  const unpriced = [...rows.values()].filter((r) => r.kind === "unpriced");

  const statements = [...rows.values()].map(
    (r) =>
      `INSERT INTO model_prices (model, source_model, source_provider, input_usd_per_mtok, output_usd_per_mtok, cache_read_usd_per_mtok, cache_write_usd_per_mtok, input_includes_cache_read, price_kind, updated_at) VALUES (${sqlStr(r.model)}, ${sqlStr(r.sourceModel)}, ${sqlStr(r.sourceProvider)}, ${sqlNum(r.input)}, ${sqlNum(r.output)}, ${sqlNum(r.cacheRead)}, ${sqlNum(r.cacheWrite)}, ${r.cacheInclusive}, ${sqlStr(r.kind)}, datetime('now')) ON CONFLICT(model) DO UPDATE SET source_model=excluded.source_model, source_provider=excluded.source_provider, input_usd_per_mtok=excluded.input_usd_per_mtok, output_usd_per_mtok=excluded.output_usd_per_mtok, cache_read_usd_per_mtok=excluded.cache_read_usd_per_mtok, cache_write_usd_per_mtok=excluded.cache_write_usd_per_mtok, input_includes_cache_read=excluded.input_includes_cache_read, price_kind=excluded.price_kind, updated_at=datetime('now') WHERE ${PRICE_UPSERT_GUARD_SQL};`,
  );

  // A label can occur under multiple providers. Pricing is keyed by label, but
  // coverage must count every observed pair, not just its first occurrence.
  const report = buildModelPriceReport(observed, rows);
  const coverage = report.matched_token_pct;
  console.log(
    `Priced ${rows.size - unpriced.length}/${rows.size} labels; ` +
      `reported-token coverage ${coverage === null ? "n/a" : coverage.toFixed(2) + "%"}.`,
  );
  if (report.unpriced.length) {
    console.log("Top unpriced labels by tokens:");
    for (const u of report.unpriced.slice(0, 15)) {
      console.log(`  ${u.model}  ${u.reported_tokens.toLocaleString()} tokens (${u.providers.join(", ")})`);
    }
  }
  if (reportPath) {
    writeFileSync(reportPath, JSON.stringify({
      ...report, catalog_url: CATALOG_URL, generated_at: new Date().toISOString(),
      observed_days: days, dry_run: dryRun,
    }, null, 2) + "\n");
    console.log(`Wrote pricing coverage report to ${reportPath}`);
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
