/**
 * In-process mock harness server for SDK tests.
 *
 * Speaks the same NDJSON frames as the bridge so client tests exercise real
 * framing and reply correlation instead of stubbed methods.
 */

import net from "node:net";
import os from "node:os";
import path from "node:path";
import fs from "node:fs";
import { NdjsonDecoder } from "../dist/index.js";

export interface MockOptions {
  /** Called for each client request; return frames to send back. */
  onRequest?: (request: any, send: (frame: any) => void) => void;
}

export interface MockServer {
  socketPath: string;
  close(): Promise<void>;
  /** Number of currently open client connections. */
  clientCount(): number;
  /** Push an unsolicited event to every connected client. */
  broadcast(event: any): void;
}

export async function startMockHarness(options: MockOptions = {}): Promise<MockServer> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-sdk-test-"));
  const socketPath = path.join(dir, "api.sock");
  const clients = new Set<net.Socket>();

  const server = net.createServer((socket) => {
    clients.add(socket);
    socket.on("close", () => clients.delete(socket));
    socket.on("error", () => clients.delete(socket));
    const decoder = new NdjsonDecoder();
    socket.on("data", (chunk) => {
      for (const raw of decoder.push(chunk)) {
        const request = raw as any;
        const send = (frame: any) => socket.write(`${JSON.stringify(frame)}\n`);
        if (request.req === "hello") {
          send({
            v: 1,
            reply_to: request.id,
            ev: "hello_ok",
            version: 1,
            server: "mock/0.1",
            capabilities: ["sessions", "streaming"],
          });
          continue;
        }
        options.onRequest?.(request, send);
      }
    });
  });

  await new Promise<void>((resolve) => server.listen(socketPath, resolve));

  return {
    socketPath,
    clientCount() {
      return clients.size;
    },
    broadcast(event: any) {
      for (const socket of clients) socket.write(`${JSON.stringify(event)}\n`);
    },
    close() {
      for (const socket of clients) socket.destroy();
      return new Promise<void>((resolve) => {
        server.close(() => {
          fs.rmSync(dir, { recursive: true, force: true });
          resolve();
        });
      });
    },
  };
}
