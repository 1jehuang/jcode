#!/usr/bin/env node
// One-off mover: copy BULK_EVENTS rows (and their detail rows) from the main
// telemetry D1 into jcode-telemetry-bulk, then delete them from main.
// Resumable: progress is keyed by the main-DB events.id cursor per family, and
// inserts are INSERT OR IGNORE on event_id, so re-running is safe.
//
// Usage: CLOUDFLARE_API_TOKEN=... node scripts/move-bulk-events.mjs [--dry-run]

import { readFileSync, writeFileSync, existsSync } from "node:fs";

const ACCOUNT = "9d028e2eacd63c11cbc7df1f9c7a43e4";
const MAIN = "abaa524c-3e90-4ba9-a569-027e78a083c6";
const BULK = "e7bb5e59-4a72-4dd2-b2c2-bcde2ebe011a";
const FAMILIES = {
  prompt_submitted: null,
  // todo_session_details was never migrated on main; only bulk has it.
  todo_session: null,
  session_concurrency: "concurrency_details",
};
const PAGE = Number(process.env.PAGE || 200);
const STATE = process.env.STATE || "move-bulk-state.json";
const DRY = process.argv.includes("--dry-run");
const TOKEN = process.env.CLOUDFLARE_API_TOKEN;
if (!TOKEN) throw new Error("CLOUDFLARE_API_TOKEN required");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function q(db, sql, params = []) {
  for (let attempt = 0; ; attempt++) {
    const res = await fetch(
      `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}/d1/database/${db}/query`,
      { method: "POST", headers: { Authorization: `Bearer ${TOKEN}`, "Content-Type": "application/json" },
        body: JSON.stringify({ sql, params }) },
    );
    const json = await res.json().catch(() => null);
    if (process.env.DEBUG) console.error(`${db.slice(0, 4)} ${Math.round(json?.result?.at(-1)?.meta?.duration ?? -1)}ms ok=${json?.success} ${sql.slice(0, 60).replace(/\s+/g, " ")} ${json?.success ? "" : JSON.stringify(json?.errors)}`);
    if (json?.success) return json.result.at(-1).results ?? [];
    const msg = JSON.stringify(json?.errors ?? res.status);
    // Only overload/transient errors are worth retrying.
    if (json?.errors?.some((e) => e.code === 7500)) throw new Error(msg);
    if (attempt >= 20) throw new Error(msg);
    await sleep(Math.min(30000, 1000 * 2 ** Math.min(attempt, 5)));
  }
}

function lit(v) {
  if (v === null || v === undefined) return "NULL";
  if (typeof v === "number") return Number.isFinite(v) ? String(v) : "NULL";
  return `'${String(v).replace(/'/g, "''")}'`;
}

function insertSql(table, rows, drop = []) {
  const cols = Object.keys(rows[0]).filter((c) => !drop.includes(c));
  const values = rows.map((r) => `(${cols.map((c) => lit(r[c])).join(",")})`).join(",\n");
  return `INSERT OR IGNORE INTO ${table} (${cols.join(",")}) VALUES\n${values}`;
}

// Keep statements under D1's 100 KB SQL limit.
async function insertChunked(db, table, rows, drop) {
  let chunk = [];
  let size = 0;
  for (const row of rows) {
    const s = JSON.stringify(row).length * 1.2 + 200;
    if (chunk.length && size + s > 80_000) {
      await q(db, insertSql(table, chunk, drop));
      chunk = []; size = 0;
    }
    chunk.push(row); size += s;
  }
  if (chunk.length) await q(db, insertSql(table, chunk, drop));
}

const state = existsSync(STATE) ? JSON.parse(readFileSync(STATE, "utf8")) : {};
for (const [event, detail] of Object.entries(FAMILIES)) {
  let cursor = state[event] ?? 0;
  let moved = 0;
  const started = Date.now();
  for (;;) {
    const rows = await q(MAIN,
      // Moved rows are deleted, so always take the oldest remaining rows via
      // idx_events_event_created_telemetry (no full sort). In dry-run use the cursor.
      `SELECT * FROM events INDEXED BY idx_events_event_created_telemetry
       WHERE event = ? AND id > ? ORDER BY event, created_at LIMIT ?`, [event, DRY ? cursor : 0, PAGE]);
    if (!rows.length) break;
    const ids = rows.map((r) => r.id);
    const eventIds = rows.map((r) => r.event_id).filter(Boolean);
    const inList = eventIds.map(lit).join(",");
    const details = detail && eventIds.length
      ? await q(MAIN, `SELECT * FROM ${detail} WHERE event_id IN (${inList})`) : [];
    if (!DRY) {
      await insertChunked(BULK, "events", rows, ["id"]);
      if (details.length) await insertChunked(BULK, detail, details);
      // Verify every row landed before deleting anything from main.
      const landed = await q(BULK,
        `SELECT COUNT(*) n FROM events WHERE event_id IN (${inList})`);
      if (landed[0].n !== eventIds.length) throw new Error(`verify failed at id ${cursor}`);
      if (detail && eventIds.length) await q(MAIN, `DELETE FROM ${detail} WHERE event_id IN (${inList})`);
      await q(MAIN, `DELETE FROM events WHERE id IN (${ids.join(",")})`);
    }
    cursor = Math.max(...ids);
    moved += rows.length;
    state[event] = cursor;
    writeFileSync(STATE, JSON.stringify(state));
    if (moved % (PAGE * 25) === 0) {
      const rate = moved / ((Date.now() - started) / 1000);
      console.log(`JCODE_PROGRESS ${JSON.stringify({ event, moved, cursor, rows_per_s: Math.round(rate) })}`);
    }
    if (DRY || (process.env.MAX_PAGES && moved >= PAGE * Number(process.env.MAX_PAGES))) break;
  }
  console.log(`${event}: moved ${moved}`);
}
console.log("done");
