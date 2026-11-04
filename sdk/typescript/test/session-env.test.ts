import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient, HarnessError } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

test("setSessionEnv rejects correlated errors and accepts the next valid map", async (t) => {
  const server = await startMockHarness({
    onRequest(req, send) {
      if (Object.keys(req.env).some((key) => key.includes("="))) {
        send({ v: 1, reply_to: req.id, ev: "error", code: "internal", message: "Invalid session environment" });
      } else {
        send({ v: 1, reply_to: req.id, ev: "ok" });
      }
    },
  });
  t.after(() => server.close());
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  t.after(() => client.close());
  await assert.rejects(client.setSessionEnv("session-1", { "A=B": "v" }), (error: unknown) => {
    assert.ok(error instanceof HarnessError);
    assert.equal(error.code, "internal");
    assert.equal(error.message, "internal: Invalid session environment");
    return true;
  });
  await client.setSessionEnv("session-1", { VALID: "v" });
});

test("setSessionEnv sends replacement maps and empty clear", async (t) => {
  const requests: any[] = [];
  const server = await startMockHarness({
    onRequest(req, send) {
      requests.push(req);
      send({ v: 1, reply_to: req.id, ev: "ok" });
    },
  });
  t.after(() => server.close());
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  t.after(() => client.close());
  for (const env of [{ TOKEN: "private-secret", EMPTY: "" }, {}]) {
    await client.setSessionEnv("session-1", env);
    const request = requests.at(-1);
    assert.equal(request.req, "set_session_env");
    assert.equal(request.session_id, "session-1");
    assert.deepEqual(request.env, env);
  }
});
