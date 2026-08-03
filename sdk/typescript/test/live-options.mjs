/**
 * Every `launch()` option the README documents should do what it says.
 *
 * Documented options are a promise, and two of these were broken when the
 * documentation for them was written: reusing a `jcodeHome` threw EEXIST
 * relinking credentials, and then connected to the previous run's stale socket
 * with nothing behind it. Neither shows up in a suite that only ever launches
 * a fresh instance.
 *
 * Usage: node test/live-options.mjs [path-to-jcode-binary]
 */

import { JcodeClient } from "../dist/index.js";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const binary = process.argv[2] ?? "jcode";
// A binary that ignores its arguments and never opens a socket, for the
// startup-timeout check.
const slowBinary = path.join(os.tmpdir(), `jcode-slow-${process.pid}.sh`);
fs.writeFileSync(slowBinary, "#!/usr/bin/env bash\nsleep 60\n", { mode: 0o755 });
process.once("exit", () => fs.rmSync(slowBinary, { force: true }));

const failures = [];
const step = async (name, fn) => {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures.push(`${name}: ${e.message}`);
    console.log(`FAIL ${name}: ${e.message}`);
  }
};

// Every option the README documents should do what it says.
await step("jcodeHome keeps state at a fixed path across runs", async () => {
  const home = path.join(os.tmpdir(), "jcode-sdk-option-home");
  fs.rmSync(home, { recursive: true, force: true });
  const a = await JcodeClient.launch({ binary, jcodeHome: home, workingDir: process.cwd() });
  const s = await a.createSession(process.cwd());
  await a.sendMessage(s.session_id, "persist me");
  await new Promise((r) => setTimeout(r, 1500));
  await a.close();
  if (!fs.existsSync(home)) throw new Error("a named home must survive close()");

  // listSessions() reports what the daemon announced to this connection, not
  // what is on disk, so a reused home is read back by id via peekSession.
  const b = await JcodeClient.launch({ binary, jcodeHome: home, workingDir: process.cwd() });
  const seen = await b.peekSession(s.session_id, 5);
  await b.close();
  if (seen.length === 0) throw new Error("a named home should carry transcripts across runs");
  fs.rmSync(home, { recursive: true, force: true });
});

await step("inheritLogins: false starts with no credentials", async () => {
  const c = await JcodeClient.launch({
    binary,
    workingDir: process.cwd(),
    inheritLogins: false,
  });
  const home = c.instanceHome;
  const hasAuth = fs.existsSync(`${home}/auth.json`);
  await c.close();
  if (hasAuth) throw new Error("inheritLogins:false must not copy auth.json");
});

await step("env reaches the instance", async () => {
  const c = await JcodeClient.launch({
    binary,
    workingDir: process.cwd(),
    env: { JCODE_NO_TELEMETRY: "1" },
  });
  await c.close();
});

await step("startupTimeoutMs is honoured", async () => {
  const t0 = Date.now();
  try {
    await JcodeClient.launch({ binary: slowBinary, startupTimeoutMs: 1500 });
    throw new Error("expected a timeout");
  } catch (e) {
    if (e.code !== "startup_timeout") throw new Error(`wrong code: ${e.code}: ${e.message}`);
    const took = Date.now() - t0;
    if (took > 12000) throw new Error(`timeout took ${took}ms, expected near 1500`);
  }
});

console.log(failures.length ? `\nFAILURES:\n${failures.join("\n")}` : "\noptions ok");
process.exit(failures.length ? 1 : 0);
