// Runtime fixture for mobile_local_backend_acceptance.rs. No production service is contacted.
import { createServer } from 'node:http';
import { DatabaseSync } from 'node:sqlite';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const root = process.argv[2];
const { default: worker } = await import(pathToFileURL(resolve(root, 'src/worker.js')));
const { hashApiKey } = await import(pathToFileURL(resolve(root, 'src/lib.js')));
const db = new DatabaseSync(':memory:');
db.exec(readFileSync(resolve(root, 'schema.sql'), 'utf8'));
for (const [account, token] of [['acceptance', 'a'], ['other', 'b']]) {
  db.prepare(`INSERT INTO accounts (account_id, email, tier, status, billing_state)
    VALUES (?, ?, 'none', 'active', 'legacy')`).run(account, `${account}@example.invalid`);
  db.prepare('INSERT INTO keys (key_id, account_id, key_hash) VALUES (?, ?, ?)')
    .run(account, account, await hashApiKey('jck_live_' + token.repeat(40)));
}
const env = { DB: {
  prepare(sql) {
    return { values: [], bind(...values) { this.values = values; return this; },
      async first() { return db.prepare(sql).get(...this.values) ?? null; },
      async all() { return { results: db.prepare(sql).all(...this.values) }; },
      async run() { return { meta: { changes: Number(db.prepare(sql).run(...this.values).changes) } }; },
    };
  },
  async batch(statements) {
    db.exec('BEGIN');
    try {
      const results = [];
      for (const statement of statements) results.push(await statement.run());
      db.exec('COMMIT');
      return results;
    } catch (error) { db.exec('ROLLBACK'); throw error; }
  },
} };
const server = createServer(async (incoming, outgoing) => {
  try {
    const chunks = [];
    for await (const chunk of incoming) chunks.push(chunk);
    const body = Buffer.concat(chunks);
    const request = new Request(`http://127.0.0.1${incoming.url}`, {
      method: incoming.method, headers: incoming.headers,
      ...(body.length ? { body } : {}),
    });
    const response = await worker.fetch(request, env, { waitUntil(promise) { promise.catch(console.error); } });
    outgoing.writeHead(response.status, Object.fromEntries(response.headers));
    outgoing.end(Buffer.from(await response.arrayBuffer()));
  } catch (error) {
    console.error(error);
    outgoing.writeHead(500);
    outgoing.end(JSON.stringify({ error: String(error) }));
  }
});
server.listen(0, '127.0.0.1', () => console.log(JSON.stringify({ port: server.address().port })));
