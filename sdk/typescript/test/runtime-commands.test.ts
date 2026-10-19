import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient, HarnessError } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

test("runtime commands revalidate stale hello capabilities without retrying mutations", async () => {
  const requests: any[] = [];
  const server = await startMockHarness({
    capabilities: [],
    onRequest(request, send) {
      requests.push(request);
      send({ v: 1, reply_to: request.id, ev: "runtime_command_result",
        session_id: request.session_id, command: request.command,
        data: { enabled: false }, message: "Memory off" });
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  try {
    assert.deepEqual(client.capabilities, []);
    const output = await client.runtimeCommand("runtime-session", "memory", ["off"]);
    assert.deepEqual(output.data, { enabled: false });
    assert.equal(requests.length, 1);
    assert.equal(requests[0].req, "runtime_command");
    assert.equal(requests[0].session_id, "runtime-session");
    assert.deepEqual(requests[0].args, ["off"]);
    assert.equal(requests[0].content, undefined);
  } finally {
    client.close();
    await server.close();
  }
});

test("runtime commands preserve an old bridge rejection without resubmitting", async () => {
  let count = 0;
  const server = await startMockHarness({
    capabilities: [],
    onRequest(request, send) {
      count++;
      send({ v: 1, reply_to: request.id, ev: "error", code: "unknown_request",
        message: "Upgrade the runtime host with jcode server reload" });
    },
  });
  const client = await JcodeClient.connect({ socketPath: server.socketPath });
  try {
    await assert.rejects(() => client.runtimeCommand("runtime-session", "skills"), (error: unknown) => {
      assert.ok(error instanceof HarnessError);
      assert.equal(error.code, "unknown_request");
      assert.match(error.message, /jcode server reload/);
      return true;
    });
    assert.equal(count, 1);
  } finally {
    client.close();
    await server.close();
  }
});
