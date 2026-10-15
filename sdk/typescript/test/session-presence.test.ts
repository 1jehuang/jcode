import { test } from "node:test";
import assert from "node:assert/strict";
import { JcodeClient, type SessionInfo } from "../dist/index.js";
import { startMockHarness } from "./mock-harness.ts";

for (const presence of [undefined, false, true]) {
  test(`session user presence survives list and attach (${presence ?? "legacy"})`, async () => {
    const session: SessionInfo = {
      session_id: "session-presence",
      status: "idle",
      ...(presence === undefined ? {} : { user_connected: presence }),
    };
    const server = await startMockHarness({
      onRequest(request, send) {
        if (request.req === "list_sessions") {
          send({ v: 1, reply_to: request.id, ev: "sessions", sessions: [session] });
        } else if (request.req === "attach_session") {
          send({ v: 1, reply_to: request.id, ev: "attached", session });
        }
      },
    });
    const client = await JcodeClient.connect({ socketPath: server.socketPath });
    try {
      const [listed] = await client.listSessions();
      const attached = await client.attachSession(session.session_id);
      for (const result of [listed, attached]) {
        assert.deepEqual(result, session);
        assert.equal(result.user_connected ?? false, presence ?? false);
        assert.equal(Object.hasOwn(result, "user_connected"), presence !== undefined);
      }
    } finally {
      client.close();
      await server.close();
    }
  });
}
