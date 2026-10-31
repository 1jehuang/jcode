import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

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
