/**
 * TypeScript SDK for the jcode harness API.
 *
 * ```ts
 * import { JcodeClient } from "@jcode/sdk";
 * const client = await JcodeClient.connect({ clientName: "my-app/1.0" });
 * const session = await client.createSession(process.cwd());
 * const turn = await client.run(session.session_id, "hello");
 * console.log(turn.text);
 * client.close();
 * ```
 */

export * from "./protocol.js";
export * from "./sockets.js";
export * from "./framing.js";
export { JcodeClient, HarnessError, unixSocketTransport } from "./client.js";
export type { ConnectOptions, Transport, TurnResult } from "./client.js";
