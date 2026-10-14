import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import worker from "../src/worker.js";

const id = "11111111-2222-4333-8444-555555555555";
const otherId = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const email = "person@example.com";
const email_hash = createHash("sha256").update(email).digest("hex");
const base = { id, provider: "openai", email_hash };
const schema = readFileSync(new URL("../schema.sql", import.meta.url), "utf8");
const migration = readFileSync(new URL("../migrations/0030_oauth_accounts.sql", import.meta.url), "utf8");
function setup() {
  const db = new DatabaseSync(":memory:");
  db.exec(schema);
  const queries = [];
  const env = {
    DB: { prepare(sql) {
      queries.push(sql);
      let values = [];
      return {
        bind(...args) { values = args; return this; },
        async first() { return db.prepare(sql).get(...values); },
        async run() { return { meta: { changes: db.prepare(sql).run(...values).changes } }; },
      };
    } },
    FIREHOSE: { writeDataPoint() { assert.fail("Private accounts must not enter firehose"); } },
  };
  return { db, env, queries };
}
function request(body = base, path = "/check", options = {}) {
  return new Request(`https://telemetry.example/v1/oauth-account${path}`, {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), ...options,
  });
}
async function call(env, body = base, path = "/check", options = {}) {
  return worker.fetch(request(body, path, options), env, {});
}
function known(db, source = "events") {
  if (source === "events") db.prepare("INSERT INTO events (telemetry_id,event,version,os,arch) VALUES (?, 'install', 'test', 'linux', 'x86_64')").run(id);
  else db.prepare("INSERT INTO daily_active_users (activity_date,telemetry_id) VALUES ('2026-10-14', ?)").run(id);
}

test("digest preflight is private and cannot expose another installation's email membership", async () => {
  const { db, env } = setup();
  db.prepare("INSERT INTO oauth_accounts (telemetry_id,provider,email,email_hash) VALUES (?, ?, ?, ?)").run(otherId, "openai", email, email_hash);
  const unknown = await call(env);
  assert.deepEqual(await unknown.json(), { installation_known: false, email_tracked: false });
  assert.equal(unknown.headers.get("Cache-Control"), "no-store");
  assert.equal(unknown.headers.get("Access-Control-Allow-Origin"), null);
  known(db);
  assert.deepEqual(await (await call(env)).json(), { installation_known: true, email_tracked: false });
  assert.deepEqual(await (await call(env, { ...base, id: otherId })).json(), { installation_known: true, email_tracked: true });
  assert.deepEqual(await (await call(env, { ...base, provider: "anthropic" })).json(), { installation_known: true, email_tracked: false });
});

test("installation identity survives raw-event deletion in durable daily activity", async () => {
  const { db, env } = setup();
  known(db, "daily_active_users");
  assert.deepEqual(await (await call(env)).json(), { installation_known: true, email_tracked: false });
});

test("upload accepts new installations, canonicalizes email, and atomically deduplicates within installations without refreshing", async () => {
  const { db, env } = setup();
  const response = await call(env, { ...base, email: " Person@EXAMPLE.COM " }, "");
  assert.deepEqual(await response.json(), { ok: true });
  const row = db.prepare("SELECT * FROM oauth_accounts").get();
  assert.equal(row.email, email);
  assert.equal(row.email_hash, email_hash);
  assert.equal(row.telemetry_id, id);
  assert.deepEqual(await (await call(env, { ...base, email }, "")).json(), { ok: true });
  assert.deepEqual(db.prepare("SELECT * FROM oauth_accounts").get(), row);
  assert.deepEqual(await (await call(env, { ...base, id: otherId, email }, "")).json(), { ok: true });
  assert.deepEqual(await (await call(env)).json(), { installation_known: true, email_tracked: true });
  assert.deepEqual(await (await call(env, { id, provider: "anthropic", email }, "")).json(), { ok: true });
  assert.equal(db.prepare("SELECT COUNT(*) AS count FROM oauth_accounts").get().count, 3);
});

test("strict payload validation rejects malformed IDs, unsupported providers, secrets, raw-email preflight, hashes and oversized bodies before DB access", async () => {
  for (const [body, path] of [
    [{ ...base, id: "not-uuid" }, "/check"],
    [{ ...base, provider: "gmail" }, "/check"],
    [{ ...base, email }, "/check"],
    [{ ...base, email_hash: email_hash.toUpperCase() }, "/check"],
    [{ ...base, email_hash: "z".repeat(64) }, "/check"],
    [{ ...base, access_token: "secret" }, "/check"],
    [{ ...base, email, refresh_token: "secret" }, ""],
    [{ ...base, email, email_hash: "0".repeat(64) }, ""],
    [{ ...base, email: "person@example.com\nBcc: other@example.com" }, ""],
    [{ ...base, email: "Display Name <person@example.com>" }, ""],
    [{ ...base, email: ".person@example.com" }, ""],
    [{ ...base, email: "person.@example.com" }, ""],
    [{ ...base, email: "person@example-.com" }, ""],
    [{ ...base, email: "person@example.-com" }, ""],
    [{ ...base, email: "x".repeat(65) + "@example.com" }, ""],
    [null, "/check"], [[], "/check"],
  ]) {
    const { env, queries } = setup();
    assert.equal((await call(env, body, path)).status, 400);
    assert.deepEqual(queries, []);
  }
  const { env, queries } = setup();
  assert.equal((await call(env, { ...base, padding: "x".repeat(1100) })).status, 413);
  assert.deepEqual(queries, []);
});

test("model provider allowlist is supported and does not include unrelated OAuth identities", async () => {
  const { env } = setup();
  for (const provider of ["anthropic", "openai", "gemini", "copilot", "antigravity"]) {
    assert.equal((await call(env, { ...base, provider })).status, 200);
  }
});

test("private routes refuse browser origins, CORS preflights and GETs", async () => {
  const { env, queries } = setup();
  for (const path of ["", "/check"]) {
    const origin = await call(env, base, path, { headers: { "Content-Type": "application/json", Origin: "https://jcode.sh" } });
    assert.equal(origin.status, 403);
    assert.equal(origin.headers.get("Access-Control-Allow-Origin"), null);
    for (const method of ["OPTIONS", "GET"]) {
      const response = await worker.fetch(new Request(`https://telemetry.example/v1/oauth-account${path}`, { method }), env, {});
      assert.equal(response.status, 405);
      assert.equal(response.headers.get("Access-Control-Allow-Origin"), null);
    }
  }
  assert.deepEqual(queries, []);
});

test("invalid JSON and media types are rejected without database calls", async () => {
  const { env, queries } = setup();
  assert.equal((await call(env, base, "/check", { body: "{" })).status, 400);
  assert.equal((await call(env, base, "/check", { headers: { "Content-Type": "text/plain" } })).status, 415);
  assert.deepEqual(queries, []);
});

test("database failures are generic and never expose email or hash", async () => {
  const env = { DB: { prepare() { throw new Error(`private ${email} ${email_hash}`); } } };
  for (const [body, path] of [[base, "/check"], [{ ...base, email }, ""]]) {
    const response = await call(env, body, path);
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: "Account storage unavailable" });
  }
});

test("streamed body cap uses actual bytes, not a spoofed Content-Length", async () => {
  const { env, queries } = setup();
  let cancelled = false;
  let chunk = 0;
  const stream = new ReadableStream({
    pull(controller) {
      controller.enqueue(new Uint8Array(600).fill(32));
      if (++chunk === 4) controller.close();
    },
    cancel() { cancelled = true; },
  });
  const response = await worker.fetch(new Request("https://telemetry.example/v1/oauth-account/check", {
    method: "POST", headers: { "Content-Type": "application/json", "Content-Length": "10" },
    body: stream, duplex: "half",
  }), env, {});
  assert.equal(response.status, 413);
  assert.equal(cancelled, true);
  assert.deepEqual(queries, []);
});

test("concurrent uploads produce one installation link and deletion removes email and digest together", async () => {
  const { db, env } = setup();
  known(db);
  const responses = await Promise.all(Array.from({ length: 10 }, () => call(env, { ...base, email }, "")));
  for (const response of responses) assert.deepEqual(await response.json(), { ok: true });
  assert.equal(db.prepare("SELECT COUNT(*) AS count FROM oauth_accounts").get().count, 1);
  db.prepare("DELETE FROM oauth_accounts WHERE telemetry_id = ?").run(id);
  assert.equal(db.prepare("SELECT COUNT(*) AS count FROM oauth_accounts").get().count, 0);
  assert.deepEqual(await (await call(env)).json(), { installation_known: true, email_tracked: false });
});

test("additive migration is idempotent and matches fresh private-account schema", () => {
  const fresh = new DatabaseSync(":memory:");
  fresh.exec(schema);
  const upgraded = new DatabaseSync(":memory:");
  upgraded.exec("CREATE TABLE daily_active_users (telemetry_id TEXT)");
  upgraded.exec(migration);
  upgraded.exec(migration);
  const sql = "SELECT name,sql FROM sqlite_master WHERE name LIKE '%oauth_accounts%' ORDER BY name";
  assert.deepEqual(upgraded.prepare(sql).all(), fresh.prepare(sql).all());
  assert.throws(() => upgraded.prepare("INSERT INTO oauth_accounts (telemetry_id,provider,email,email_hash) VALUES (?, ?, ?, ?)").run(id, "gmail", email, email_hash));
  assert.throws(() => upgraded.prepare("INSERT INTO oauth_accounts (telemetry_id,provider,email,email_hash) VALUES (?, ?, ?, ?)").run(id, "openai", email, "z".repeat(64)));
});

test("nightly scheduled retention removes only expired private account records", async () => {
  const { db, env, queries } = setup();
  db.prepare("INSERT INTO oauth_accounts (telemetry_id,provider,email,email_hash,created_at) VALUES (?, ?, ?, ?, datetime('now', '-91 days'))").run(id, "openai", email, email_hash);
  db.prepare("INSERT INTO oauth_accounts (telemetry_id,provider,email,email_hash) VALUES (?, ?, ?, ?)").run(id, "anthropic", email, email_hash);
  // Other cron facilities have their own tests. Make them no-ops in this adapter.
  const prepare = env.DB.prepare;
  env.DB.prepare = sql => sql.includes("DELETE FROM oauth_accounts") ? prepare(sql) : {
    bind() { return this; }, async run() { return { meta: { changes: 0 } }; }, async all() { return { results: [] }; },
  };
  const waited = [];
  await worker.scheduled({}, env, { waitUntil(promise) { waited.push(promise); } });
  await Promise.all(waited);
  assert.equal(db.prepare("SELECT COUNT(*) AS count FROM oauth_accounts").get().count, 1);
  assert.equal(db.prepare("SELECT provider FROM oauth_accounts").get().provider, "anthropic");
  assert.equal(queries.filter(sql => sql.includes("DELETE FROM oauth_accounts")).length, 1);
});
