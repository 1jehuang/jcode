#!/usr/bin/env node
// Render a dashboard .sql file as a readable table.
//
// Why this exists: `wrangler d1 execute --file=...` uses D1's *import* path,
// which reports only "Rows read / Rows written / Database size" and throws the
// actual result set away. Every dashboard here was wired through `--file`, so
// `npm run health` printed counters and no data. `--command` returns real rows,
// so this runner reads the .sql file and sends it as a command instead, then
// formats the output.
//
// Usage:
//   node scripts/run-dashboard.mjs token-value.sql
//   node scripts/run-dashboard.mjs health.sql --json
//
// Only single-statement dashboard queries are supported, which is what
// `--command` accepts and what all the dashboard files are.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const DB = "jcode-telemetry";
const args = process.argv.slice(2);
const asJson = args.includes("--json");
const file = args.find((a) => !a.startsWith("--"));

if (!file) {
  console.error("usage: node scripts/run-dashboard.mjs <file.sql> [--json]");
  process.exit(1);
}

const sql = readFileSync(file, "utf8");

const raw = execFileSync(
  "npx",
  // `--command=<sql>` rather than `--command <sql>`: dashboard files open with
  // a `--` SQL comment, which yargs would otherwise parse as CLI flags.
  ["wrangler", "d1", "execute", DB, "--remote", "--json", `--command=${sql}`],
  { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
);

const start = raw.indexOf("[");
if (start < 0) {
  console.error(raw);
  process.exit(1);
}
const results = JSON.parse(raw.slice(start))[0]?.results ?? [];

if (asJson) {
  console.log(JSON.stringify(results, null, 2));
  process.exit(0);
}

if (!results.length) {
  console.log("(no rows)");
  process.exit(0);
}

function format(value) {
  if (value === null || value === undefined) return "";
  if (typeof value === "number") {
    return Number.isInteger(value) ? value.toLocaleString("en-US") : value.toFixed(2);
  }
  return String(value);
}

const columns = [...new Set(results.flatMap((row) => Object.keys(row)))];
const widths = columns.map((column) =>
  Math.max(column.length, ...results.map((row) => format(row[column]).length)),
);

// Right-align numbers, left-align labels, matching the column's dominant type.
const numeric = columns.map((column) =>
  results.every((row) => row[column] === null || typeof row[column] === "number"),
);

const pad = (text, width, right) => (right ? text.padStart(width) : text.padEnd(width));

console.log(columns.map((c, i) => pad(c, widths[i], numeric[i])).join("  "));
console.log(widths.map((w) => "-".repeat(w)).join("  "));
for (const row of results) {
  console.log(columns.map((c, i) => pad(format(row[c]), widths[i], numeric[i])).join("  "));
}
