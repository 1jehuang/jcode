/**
 * Isolated end-to-end regression for #2049, with no model requests.
 * Run after building the SDK and TUI: node test/live-session-env.mjs
 * JCODE_BIN can select a different binary. The shared daemon is never touched.
 */
import { spawn } from "node:child_process";
import { access, mkdir, mkdtemp, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { JcodeClient, HarnessError } from "../dist/index.js";

const binary = process.env.JCODE_BIN ?? fileURLToPath(new URL("../../../target/selfdev/jcode", import.meta.url));
await access(binary);
const scratch = process.env.JCODE_SCRATCH_DIR ?? join(homedir(), ".jcode", "scratch");
await mkdir(scratch, { recursive: true });
const root = await mkdtemp(join(scratch, "session-env-e2e-"));
const socket = join(root, "daemon.sock");
const api = join(root, "api.sock");
const env = { ...process.env, JCODE_HOME: root, JCODE_SOCKET: socket, JCODE_NON_INTERACTIVE: "1" };
const children = [];
let diagnostics = "";
function launch(args) {
  const child = spawn(binary, ["--no-update", "--socket", socket, ...args], { env, stdio: ["ignore", "pipe", "pipe"] });
  child.stdout.on("data", (chunk) => { diagnostics += chunk; });
  child.stderr.on("data", (chunk) => { diagnostics += chunk; });
  child.on("error", (error) => { diagnostics += error.message; });
  children.push(child);
}
async function waitSocket(path) {
  for (let n = 0; n < 200; n++) {
    if (await stat(path).then((entry) => entry.isSocket(), () => false)) return;
    await delay(100);
  }
  throw new Error(`Timed out waiting for isolated socket: ${diagnostics}`);
}
let client;
try {
  launch(["serve", "--temporary-server", "--owner-pid", String(process.pid), "--temp-idle-timeout-secs", "30"]);
  await waitSocket(socket);
  launch(["api-bridge", "--api-socket", api]);
  await waitSocket(api);
  client = await JcodeClient.connect({ socketPath: api, requestTimeoutMs: 5000 });
  const session = await client.createSession({ workingDir: root });
  for (const invalid of [{ "BAD=KEY": "v" }, { BAD: "nul\0value" }, { JCODE_SESSION_ID: "x" }, { "": "v" }]) {
    await assert.rejects(client.setSessionEnv(session.session_id, invalid), (error) => error instanceof HarnessError && error.code === "internal");
  }
  await client.setSessionEnv(session.session_id, { VALID: "v" });
  await client.setSessionEnv(session.session_id, {});
  await client.setSessionSaved(session.session_id, true, "isolated-env-smoke");
  await client.setSessionSaved(session.session_id, false);
  console.log("PASS: real daemon, bridge and SDK reject invalid environment maps and accept valid/clear/save controls");
} catch (error) {
  console.error(diagnostics);
  throw error;
} finally {
  client?.close();
  for (const child of children.reverse()) child.kill("SIGTERM");
  await Promise.all(children.map((child) => child.exitCode !== null || child.signalCode !== null || !child.pid
    ? undefined : new Promise((resolve) => child.once("exit", resolve))));
}
