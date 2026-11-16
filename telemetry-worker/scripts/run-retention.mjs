#!/usr/bin/env node
// Run the same bounded hourly retention pass as production, without waiting
// for cron. Defaults to read-only health. --apply expires only data covered by
// the worker's existing retention policy. Never VACUUM or shorten retention.
import { readFileSync } from "node:fs";
import worker from "../src/worker.js";

const apply = process.argv.includes("--apply");
if (process.argv.slice(2).some((arg) => arg !== "--apply")) {
  throw new Error("Usage: node scripts/run-retention.mjs [--apply]");
}
const health = async () => {
  const response = await fetch("https://telemetry.jcode.sh/v1/health", {
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) throw new Error(`Health HTTP ${response.status}`);
  const result = await response.json();
  console.log(JSON.stringify(result));
  return result;
};
await health();
if (!apply) {
  console.log("Read-only. Pass --apply to run one bounded existing-policy retention pass.");
} else {
  const account = "9d028e2eacd63c11cbc7df1f9c7a43e4";
  const config = readFileSync(new URL("../wrangler.toml", import.meta.url), "utf8");
  const bindings = [...config.matchAll(/\[\[d1_databases\]\]\s*binding = "([^"]+)"\s*database_name = "[^"]+"\s*database_id = "([^"]+)"/g)];
  if (!bindings.some(([, name]) => name === "DB") || !bindings.some(([, name]) => name === "DB_BULK")) {
    throw new Error("Missing main/bulk D1 configuration");
  }
  const token = () => process.env.CLOUDFLARE_API_TOKEN || readFileSync(
    `${process.env.HOME}/.wrangler/config/default.toml`, "utf8",
  ).match(/^oauth_token = "(.*)"/m)?.[1];
  if (!token()) throw new Error("Cloudflare authentication required");
  let statements = 0;
  let changed = 0;
  let failures = 0;
  let retries = 0;
  const blocked = new Map();
  const env = Object.fromEntries(bindings.map(([, name, id]) => [name, {
    prepare(sql) {
      const execute = async (params = []) => {
        try {
          if (blocked.has(id)) throw blocked.get(id);
          let body;
          for (let attempt = 0; ; attempt++) {
            const response = await fetch(`https://api.cloudflare.com/client/v4/accounts/${account}/d1/database/${id}/query`, {
              method: "POST",
              headers: { Authorization: `Bearer ${token()}`, "Content-Type": "application/json" },
              body: JSON.stringify({ sql, params }),
              signal: AbortSignal.timeout(120_000),
            });
            body = await response.json();
            const overloaded = response.status === 429 || response.status === 503
              || body.errors?.some((error) => error.code === 7429);
            if (overloaded && attempt < 3) {
              retries++;
              console.warn(`${name}: D1 overloaded, backing off before retry ${attempt + 1}/3`);
              await new Promise((resolve) => setTimeout(resolve, 11_000 * (attempt + 1)));
              continue;
            }
            if (!response.ok || !body.success || !body.result?.every((result) => result.success)) {
              const error = new Error(`D1 ${name}: ${JSON.stringify(body.errors || response.status)}`);
              if (overloaded) blocked.set(id, error);
              throw error;
            }
            break;
          }
          const result = body.result.at(-1);
          statements++;
          changed += result.meta?.changes || 0;
          if (result.meta?.changes) console.log(`${name}: ${result.meta.changes} rows changed`);
          return result;
        } catch (error) {
          failures++;
          throw error;
        }
      };
      return { run: () => execute(), bind: (...params) => ({ run: () => execute(params) }) };
    },
  }]));
  // Use the hourly cron identity to skip unrelated nightly maintenance.
  const pending = [];
  await worker.scheduled({ cron: "17 0-3,5-23 * * *" }, env, {
    waitUntil(promise) { pending.push(promise); },
  });
  await Promise.all(pending);
  console.log(JSON.stringify({ statements, changed, failures, retries }));
  await health();
  if (failures) process.exitCode = 1;
}
