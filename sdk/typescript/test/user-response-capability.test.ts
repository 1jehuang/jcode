import { test } from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import fs from "node:fs";
import { JcodeClient, NdjsonDecoder } from "../dist/index.js";

test("inline user response capability is explicitly opted in at hello", async () => {
  for (const enabled of [false, true]) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-response-test-"));
    const socketPath = path.join(dir, "api.sock");
    let hello: any;
    const server = net.createServer((socket) => {
      const decoder = new NdjsonDecoder();
      socket.on("data", (chunk) => {
        for (const frame of decoder.push(chunk)) {
          hello = frame;
          socket.write(`${JSON.stringify({
            v: 1, reply_to: hello.id, ev: "hello_ok", version: 1,
            server: "response-test", capabilities: [],
          })}\n`);
        }
      });
    });
    await new Promise<void>((resolve) => server.listen(socketPath, resolve));
    let client: JcodeClient | undefined;
    try {
      client = await JcodeClient.connect({
        socketPath,
        ...(enabled ? { supportsUserResponse: true } : {}),
      });
      assert.equal(hello.req, "hello");
      assert.equal(hello.supports_user_response ?? false, enabled);
    } finally {
      client?.close();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }
});
